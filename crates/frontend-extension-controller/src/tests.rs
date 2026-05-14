use k8s_openapi::{
    ByteString, api::batch::v1::JobStatus, apimachinery::pkg::apis::meta::v1::OwnerReference,
};

use super::*;

fn test_time(value: &str) -> Time {
    serde_json::from_value(json!(value)).unwrap()
}

fn artifact_cm(name: &str, fe_uid: &str, created_at: &str) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            creation_timestamp: Some(test_time(created_at)),
            owner_references: Some(vec![OwnerReference {
                api_version: "frontend-forge.kubesphere.io/v1alpha1".to_string(),
                kind: "FrontendExtension".to_string(),
                name: "inspecttask".to_string(),
                uid: fe_uid.to_string(),
                controller: Some(true),
                block_owner_deletion: None,
            }]),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn sample_fe() -> FrontendExtension {
    serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
            "uid": "fe-uid",
            "generation": 7,
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": { "en": "Inspect Task" },
                "description": { "en": "InspectTask extension package" }
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {}
                }
            }
        }
    }))
    .unwrap()
}

fn test_config() -> ControllerConfig {
    ControllerConfig {
        work_namespace: "extension-frontend-forge".to_string(),
        packager_image: "packager:latest".to_string(),
        packager_service_account: None,
        publisher_image: "publisher:latest".to_string(),
        publisher_service_account: Some("publisher-sa".to_string()),
        artifact_configmap_namespace: "extension-frontend-forge".to_string(),
        build_service_base_url: "http://frontend-forge.test".to_string(),
        build_service_timeout_seconds: 240,
        jsbundle_config_key: "index.js".to_string(),
        reconcile_requeue_seconds: 5,
        job_active_deadline_seconds: 300,
        job_ttl_seconds_after_finished: Some(3600),
        artifact_retain_old_count: 1,
        package_max_attempts: 3,
    }
}

#[test]
fn rebuild_token_is_trimmed_and_empty_tokens_are_equivalent() {
    let mut fe = sample_fe();
    fe.metadata.annotations = Some(BTreeMap::from([(
        ANNO_REBUILD_TOKEN.to_string(),
        " token-1 ".to_string(),
    )]));

    assert_eq!(frontend_extension_rebuild_token(&fe), "token-1");

    fe.metadata.annotations = Some(BTreeMap::from([(
        ANNO_REBUILD_TOKEN.to_string(),
        "   ".to_string(),
    )]));

    assert_eq!(frontend_extension_rebuild_token(&fe), "");
}

#[test]
fn package_job_selector_contains_fe_uid_artifact_key_and_kind() {
    let fe = sample_fe();
    let artifact_key = "sha256:0123456789abcdef0123456789abcdef";
    let selector = package_job_selector(&fe, artifact_key);

    assert!(selector.contains(&format!("{LABEL_FE_NAME}=inspecttask")));
    assert!(selector.contains(&format!("{LABEL_FE_UID}=fe-uid")));
    assert!(selector.contains(&format!(
        "{LABEL_ARTIFACT_KEY_SHORT}={}",
        hash_label_value(artifact_key)
    )));
    assert!(selector.contains(&format!("{LABEL_PACKAGE_KIND}={PACKAGE_KIND_VALUE}")));
}

#[test]
fn package_attempt_parses_positive_suffix_only() {
    let job = Job {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-package-d46b92fa1234-a12".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let invalid = Job {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-package-d46b92fa1234".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let zero = Job {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-package-d46b92fa1234-a0".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(package_attempt_from_job(&job), Some(12));
    assert_eq!(package_attempt_from_job(&invalid), None);
    assert_eq!(package_attempt_from_job(&zero), None);
}

#[test]
fn latest_package_attempt_uses_highest_parsed_attempt() {
    let attempt_1 = PackageAttempt {
        attempt: 1,
        job: Job {
            metadata: ObjectMeta {
                name: Some("fe-inspecttask-package-hash-a1".to_string()),
                creation_timestamp: Some(test_time("2026-04-20T10:01:00Z")),
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let attempt_3 = PackageAttempt {
        attempt: 3,
        job: Job {
            metadata: ObjectMeta {
                name: Some("fe-inspecttask-package-hash-a3".to_string()),
                creation_timestamp: Some(test_time("2026-04-20T10:00:00Z")),
                ..Default::default()
            },
            ..Default::default()
        },
    };

    let latest = latest_package_attempt(vec![attempt_1, attempt_3]).unwrap();

    assert_eq!(latest.attempt, 3);
    assert_eq!(latest.job.name_any(), "fe-inspecttask-package-hash-a3");
}

#[test]
fn package_attempts_exceeded_message_includes_attempt_context() {
    let message = package_attempts_exceeded_message("sha256:d46b92fa1234abcdef", 3, 3, "boom");

    assert!(message.contains("d46b92fa1234abcdef"));
    assert!(message.contains("latest attempt 3"));
    assert!(message.contains("max attempts 3"));
    assert!(message.contains("boom"));
}

#[test]
fn package_job_has_rebuild_identity_labels_annotations_and_env() {
    let fe = sample_fe();
    let config = test_config();
    let job = make_package_job(
        &fe,
        &config,
        "fe-inspecttask-package-d46b92fa1234-a1",
        "sha256:source",
        "sha256:artifactkey",
        "token-1",
        "fe-inspecttask-d46b92fa1234",
    );
    let labels = job.metadata.labels.as_ref().unwrap();
    let annotations = job.metadata.annotations.as_ref().unwrap();
    let env = job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0]
        .env
        .as_ref()
        .unwrap()
        .iter()
        .map(|env| (env.name.as_str(), env.value.as_deref().unwrap_or_default()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(labels[LABEL_FE_NAME], "inspecttask");
    assert_eq!(labels[LABEL_FE_UID], "fe-uid");
    assert_eq!(labels[LABEL_SOURCE_HASH_SHORT], "source");
    assert_eq!(labels[LABEL_ARTIFACT_KEY_SHORT], "artifactkey");
    assert_eq!(annotations[ANNO_SOURCE_HASH], "sha256:source");
    assert_eq!(annotations[ANNO_ARTIFACT_KEY], "sha256:artifactkey");
    assert_eq!(env["FE_UID"], "fe-uid");
    assert_eq!(env["ARTIFACT_KEY"], "sha256:artifactkey");
    assert_eq!(env["REBUILD_TOKEN"], "token-1");
}

#[test]
fn artifact_configmap_requires_matching_artifact_key_annotation() {
    let bytes = vec![1, 2, 3];
    let digest = format!("sha256:{}", sha256_hex(&bytes));
    let metadata = PackageArtifactMetadata {
        name: "inspecttask".to_string(),
        version: "0.1.0".to_string(),
        filename: "inspecttask-0.1.0.tgz".to_string(),
        media_type: "application/gzip".to_string(),
        digest: digest.clone(),
        size_bytes: 3,
        source_hash: "sha256:source".to_string(),
        generated_at: Utc::now(),
    };
    let mut cm = ConfigMap {
        metadata: ObjectMeta {
            annotations: Some(BTreeMap::from([
                (ANNO_SOURCE_HASH.to_string(), "sha256:source".to_string()),
                (
                    ANNO_ARTIFACT_KEY.to_string(),
                    "sha256:artifactkey".to_string(),
                ),
                (ANNO_ARTIFACT_DIGEST.to_string(), digest),
            ])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            ARTIFACT_METADATA_KEY.to_string(),
            serde_json::to_string(&metadata).unwrap(),
        )])),
        binary_data: Some(BTreeMap::from([(
            PACKAGE_KEY.to_string(),
            ByteString(bytes),
        )])),
        ..Default::default()
    };

    assert!(artifact_metadata_from_configmap(&cm, "sha256:source", "sha256:artifactkey").is_some());

    cm.metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(ANNO_ARTIFACT_KEY.to_string(), "sha256:other".to_string());

    assert!(artifact_metadata_from_configmap(&cm, "sha256:source", "sha256:artifactkey").is_none());
}

#[test]
fn publish_status_is_retained_only_for_same_artifact_key() {
    let mut fe = sample_fe();
    fe.status = Some(FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        observed_generation: Some(7),
        observed_source_hash: Some("sha256:source".to_string()),
        observed_rebuild_token: Some("token-1".to_string()),
        artifact: Some(ExtensionArtifactStatus {
            storage: ArtifactStorageStatus {
                kind: ArtifactStorageKind::ConfigMap,
                ref_: NamespacedResourceRef {
                    namespace: "extension-frontend-forge".to_string(),
                    name: "fe-inspecttask-d46b92fa1234".to_string(),
                    uid: None,
                },
                key: PACKAGE_KEY.to_string(),
            },
            digest: "sha256:artifact".to_string(),
            size_bytes: 1,
            media_type: "application/gzip".to_string(),
            filename: "inspecttask-0.1.0.tgz".to_string(),
            generated_at: Utc::now(),
            source_hash: "sha256:source".to_string(),
            artifact_key: Some("sha256:artifactkey".to_string()),
        }),
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: true,
            artifact_digest: Some("sha256:artifact".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(
        retained_publish_for_artifact_key(&fe, "sha256:artifactkey")
            .unwrap()
            .phase,
        PublishPhase::Succeeded
    );
    assert_eq!(
        retained_publish_for_artifact_key(&fe, "sha256:newkey")
            .unwrap()
            .phase,
        PublishPhase::NotRequested
    );
}

#[test]
fn current_job_status_overrides_existing_package_job() {
    let fe: FrontendExtension = serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": {
                    "en": "Inspect Task",
                },
                "description": {
                    "en": "InspectTask extension package",
                },
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {
                        "menus": [{
                            "displayName": "Inspect Tasks",
                            "key": "inspecttasks",
                            "placement": "cluster",
                            "type": "page",
                        }],
                        "pages": [{
                            "key": "inspecttasks",
                            "type": "iframe",
                            "iframe": {
                                "src": "http://example.test",
                            },
                        }],
                    },
                },
            },
        },
        "status": {
            "phase": "Ready",
            "packageJob": {
                "namespace": "extension-frontend-forge",
                "name": "fe-inspecttask-package-oldhash",
                "phase": "Running",
            },
        },
    }))
    .unwrap();

    let job = Job {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-package-newhash".to_string()),
            namespace: Some("extension-frontend-forge".to_string()),
            uid: Some("job-uid".to_string()),
            ..Default::default()
        },
        status: Some(JobStatus {
            succeeded: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };

    let package_job = current_or_existing_package_job(Some(&job), &fe).unwrap();

    assert_eq!(package_job.name, "fe-inspecttask-package-newhash");
    assert_eq!(package_job.phase, PackageJobPhase::Succeeded);
}

#[test]
fn existing_package_job_is_fallback_when_current_job_missing() {
    let fe: FrontendExtension = serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": {
                    "en": "Inspect Task",
                },
                "description": {
                    "en": "InspectTask extension package",
                },
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {
                        "menus": [{
                            "displayName": "Inspect Tasks",
                            "key": "inspecttasks",
                            "placement": "cluster",
                            "type": "page",
                        }],
                        "pages": [{
                            "key": "inspecttasks",
                            "type": "iframe",
                            "iframe": {
                                "src": "http://example.test",
                            },
                        }],
                    },
                },
            },
        },
        "status": {
            "phase": "Ready",
            "packageJob": {
                "namespace": "extension-frontend-forge",
                "name": "fe-inspecttask-package-oldhash",
                "phase": "Succeeded",
            },
        },
    }))
    .unwrap();

    let package_job = current_or_existing_package_job(None, &fe).unwrap();

    assert_eq!(package_job.name, "fe-inspecttask-package-oldhash");
    assert_eq!(package_job.phase, PackageJobPhase::Succeeded);
}

#[test]
fn status_patch_clears_stale_package_job_message() {
    let status = FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        observed_generation: Some(1),
        observed_source_hash: Some("sha256:source".to_string()),
        observed_rebuild_token: Some(String::new()),
        artifact: None,
        download: None,
        package_job: Some(PackageJobStatus {
            namespace: "extension-frontend-forge".to_string(),
            name: "fe-inspecttask-package-newhash".to_string(),
            uid: None,
            phase: PackageJobPhase::Succeeded,
            started_at: None,
            finished_at: None,
            message: None,
        }),
        publish: None,
        unpublish: None,
        conditions: vec![],
    };

    let patch = frontend_extension_status_patch(&status, "inspecttask").unwrap();

    assert_eq!(
        patch["status"]["packageJob"]["message"],
        serde_json::Value::Null
    );
}

#[test]
fn status_patch_clears_stale_publish_last_error() {
    let status = FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        observed_generation: Some(1),
        observed_source_hash: Some("sha256:source".to_string()),
        observed_rebuild_token: Some(String::new()),
        artifact: None,
        download: None,
        package_job: None,
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: true,
            request_id: Some("request-1".to_string()),
            artifact_digest: Some("sha256:artifact".to_string()),
            job_ref: None,
            started_at: None,
            finished_at: None,
            last_error: None,
        }),
        unpublish: None,
        conditions: vec![],
    };

    let patch = frontend_extension_status_patch(&status, "inspecttask").unwrap();

    assert_eq!(
        patch["status"]["publish"]["lastError"],
        serde_json::Value::Null
    );
}

#[test]
fn status_labels_map_package_and_publish_states_for_list_filters() {
    let labels = frontend_extension_status_labels(&FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: true,
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(labels[LABEL_FE_PACKAGE_STATUS], FE_PACKAGE_STATUS_READY);
    assert_eq!(labels[LABEL_FE_PUBLISH_STATUS], FE_PUBLISH_STATUS_PUBLISHED);

    let labels = frontend_extension_status_labels(&FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Packaging,
        publish: Some(PublishStatus {
            phase: PublishPhase::Running,
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(labels[LABEL_FE_PACKAGE_STATUS], FE_PACKAGE_STATUS_PACKAGING);
    assert_eq!(
        labels[LABEL_FE_PUBLISH_STATUS],
        FE_PUBLISH_STATUS_PUBLISHING
    );

    let labels = frontend_extension_status_labels(&FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Failed,
        publish: Some(PublishStatus {
            phase: PublishPhase::Failed,
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(labels[LABEL_FE_PACKAGE_STATUS], FE_PACKAGE_STATUS_FAILED);
    assert_eq!(labels[LABEL_FE_PUBLISH_STATUS], FE_PUBLISH_STATUS_FAILED);

    let labels = frontend_extension_status_labels(&FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: false,
            ..Default::default()
        }),
        ..Default::default()
    });

    assert_eq!(
        labels[LABEL_FE_PUBLISH_STATUS],
        FE_PUBLISH_STATUS_NOT_PUBLISHED
    );
}

#[test]
fn status_labels_patch_writes_metadata_labels() {
    let status = FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let patch = frontend_extension_status_labels_patch(&status);

    assert_eq!(
        patch["metadata"]["labels"][LABEL_FE_PACKAGE_STATUS],
        FE_PACKAGE_STATUS_READY
    );
    assert_eq!(
        patch["metadata"]["labels"][LABEL_FE_PUBLISH_STATUS],
        FE_PUBLISH_STATUS_PUBLISHED
    );
    assert_eq!(
        patch["metadata"]["labels"][DEPRECATED_LABEL_FE_PACKAGE_STATUS],
        serde_json::Value::Null
    );
    assert_eq!(
        patch["metadata"]["labels"][DEPRECATED_LABEL_FE_PUBLISH_STATUS],
        serde_json::Value::Null
    );
}

#[test]
fn artifact_gc_keeps_current_status_ref_and_recent_old_artifacts() {
    let fe: FrontendExtension = serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
            "uid": "fe-uid",
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": { "en": "Inspect Task" },
                "description": { "en": "InspectTask extension package" }
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {}
                }
            }
        },
        "status": {
            "phase": "Ready",
            "artifact": {
                "storage": {
                    "kind": "ConfigMap",
                    "ref": {
                        "namespace": "extension-frontend-forge",
                        "name": "fe-inspecttask-previous",
                    },
                    "key": "package.tgz"
                },
                "digest": "sha256:previous",
                "sizeBytes": 1,
                "mediaType": "application/gzip",
                "filename": "inspecttask-0.1.0.tgz",
                "generatedAt": "2026-04-20T10:00:00Z",
                "sourceHash": "sha256:previous",
                "artifactKey": "sha256:previous-key"
            }
        }
    }))
    .unwrap();
    let current = artifact_cm("fe-inspecttask-current", "fe-uid", "2026-04-20T10:04:00Z");
    let keep_names = artifact_gc_keep_names(&fe, &current);
    let configmaps = vec![
        current,
        artifact_cm("fe-inspecttask-previous", "fe-uid", "2026-04-20T10:03:00Z"),
        artifact_cm("fe-inspecttask-retained", "fe-uid", "2026-04-20T10:02:00Z"),
        artifact_cm("fe-inspecttask-delete-1", "fe-uid", "2026-04-20T10:01:00Z"),
        artifact_cm("fe-inspecttask-delete-2", "fe-uid", "2026-04-20T10:00:00Z"),
        artifact_cm(
            "fe-inspecttask-other-owner",
            "other-uid",
            "2026-04-20T09:59:00Z",
        ),
    ];

    let delete_names = artifact_configmap_gc_candidates(configmaps, &fe, &keep_names, 1);

    assert_eq!(
        delete_names,
        vec![
            "fe-inspecttask-delete-1".to_string(),
            "fe-inspecttask-delete-2".to_string(),
        ]
    );
}

#[test]
fn artifact_gc_retain_zero_deletes_all_unreferenced_owned_artifacts() {
    let fe: FrontendExtension = serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
            "uid": "fe-uid",
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": { "en": "Inspect Task" },
                "description": { "en": "InspectTask extension package" }
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {}
                }
            }
        }
    }))
    .unwrap();
    let keep_names = BTreeSet::from(["fe-inspecttask-current".to_string()]);
    let configmaps = vec![
        artifact_cm("fe-inspecttask-current", "fe-uid", "2026-04-20T10:02:00Z"),
        artifact_cm("fe-inspecttask-delete", "fe-uid", "2026-04-20T10:01:00Z"),
    ];

    let delete_names = artifact_configmap_gc_candidates(configmaps, &fe, &keep_names, 0);

    assert_eq!(delete_names, vec!["fe-inspecttask-delete".to_string()]);
}

#[test]
fn publish_job_env_includes_artifact_filename_and_target_ref() {
    let fe: FrontendExtension = serde_json::from_value(json!({
        "apiVersion": "frontend-forge.kubesphere.io/v1alpha1",
        "kind": "FrontendExtension",
        "metadata": {
            "name": "inspecttask",
            "generation": 7,
        },
        "spec": {
            "package": {
                "version": "0.1.0",
                "displayName": {
                    "en": "Inspect Task",
                },
                "description": {
                    "en": "InspectTask extension package",
                },
            },
            "source": {
                "type": "Inline",
                "inline": {
                    "schemaVersion": "v1",
                    "frontend": {},
                },
            },
        },
    }))
    .unwrap();
    let config = ControllerConfig {
        work_namespace: "extension-frontend-forge".to_string(),
        packager_image: "packager:latest".to_string(),
        packager_service_account: None,
        publisher_image: "publisher:latest".to_string(),
        publisher_service_account: Some("publisher-sa".to_string()),
        artifact_configmap_namespace: "extension-frontend-forge".to_string(),
        build_service_base_url: "http://frontend-forge.test".to_string(),
        build_service_timeout_seconds: 240,
        jsbundle_config_key: "index.js".to_string(),
        reconcile_requeue_seconds: 5,
        job_active_deadline_seconds: 300,
        job_ttl_seconds_after_finished: Some(3600),
        artifact_retain_old_count: 1,
        package_max_attempts: 3,
    };
    let request = PublishRequest {
        request_id: "request-1".to_string(),
        artifact_digest: "sha256:artifact".to_string(),
        target_ref: NamespacedResourceRef {
            namespace: "extension-frontend-forge".to_string(),
            name: "ksbuilder-publish-config".to_string(),
            uid: None,
        },
        target_kind: "Secret".to_string(),
    };
    let artifact = PackageArtifactMetadata {
        name: "inspecttask".to_string(),
        version: "0.1.0".to_string(),
        filename: "inspecttask-0.1.0.tgz".to_string(),
        media_type: "application/gzip".to_string(),
        digest: "sha256:artifact".to_string(),
        size_bytes: 1,
        source_hash: "sha256:source".to_string(),
        generated_at: Utc::now(),
    };
    let artifact_cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-a1b2c3d4".to_string()),
            namespace: Some("extension-frontend-forge".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let job = make_publish_job(
        &fe,
        &config,
        "fe-inspecttask-publish-request",
        &request,
        &artifact,
        &artifact_cm,
    );
    let env = job.spec.unwrap().template.spec.unwrap().containers[0]
        .env
        .clone()
        .unwrap()
        .into_iter()
        .map(|env| (env.name, env.value.unwrap_or_default()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(env["FE_NAME"], "inspecttask");
    assert_eq!(env["PUBLISH_REQUEST_ID"], "request-1");
    assert_eq!(env["ARTIFACT_DIGEST"], "sha256:artifact");
    assert_eq!(
        env["ARTIFACT_CONFIGMAP_NAMESPACE"],
        "extension-frontend-forge"
    );
    assert_eq!(env["ARTIFACT_CONFIGMAP_NAME"], "fe-inspecttask-a1b2c3d4");
    assert_eq!(env["ARTIFACT_CONFIGMAP_KEY"], PACKAGE_KEY);
    assert_eq!(env["ARTIFACT_FILENAME"], "inspecttask-0.1.0.tgz");
    assert_eq!(env["PUBLISH_TARGET_KIND"], "Secret");
    assert_eq!(env["PUBLISH_TARGET_NAMESPACE"], "extension-frontend-forge");
    assert_eq!(env["PUBLISH_TARGET_NAME"], "ksbuilder-publish-config");
}

#[test]
fn unpublish_job_env_includes_action_extension_name_and_target_ref() {
    let fe = sample_fe();
    let config = test_config();
    let request = UnpublishRequest {
        request_id: "request-1".to_string(),
        extension_name: "inspecttask".to_string(),
        target_ref: NamespacedResourceRef {
            namespace: "extension-frontend-forge".to_string(),
            name: "ksbuilder-publish-config".to_string(),
            uid: None,
        },
        target_kind: "Secret".to_string(),
    };

    let job = make_unpublish_job(&fe, &config, "fe-inspecttask-unpublish-request", &request);
    let env = job.spec.unwrap().template.spec.unwrap().containers[0]
        .env
        .clone()
        .unwrap()
        .into_iter()
        .map(|env| (env.name, env.value.unwrap_or_default()))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(env["FE_NAME"], "inspecttask");
    assert_eq!(env["PUBLISH_ACTION"], "unpublish");
    assert_eq!(env["UNPUBLISH_REQUEST_ID"], "request-1");
    assert_eq!(env["UNPUBLISH_EXTENSION_NAME"], "inspecttask");
    assert_eq!(env["PUBLISH_TARGET_KIND"], "Secret");
    assert_eq!(env["PUBLISH_TARGET_NAMESPACE"], "extension-frontend-forge");
    assert_eq!(env["PUBLISH_TARGET_NAME"], "ksbuilder-publish-config");
}

#[test]
fn publish_succeeded_status_is_active() {
    let request = PublishRequest {
        request_id: "request-1".to_string(),
        artifact_digest: "sha256:artifact".to_string(),
        target_ref: NamespacedResourceRef::default(),
        target_kind: "ConfigMap".to_string(),
    };
    let job = Job {
        status: Some(JobStatus {
            succeeded: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    };

    let status = publish_status_from_job(&request, &job);

    assert_eq!(status.phase, PublishPhase::Succeeded);
    assert!(status.active);
}

#[test]
fn unpublish_success_marks_publish_inactive() {
    let mut status = FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        publish: Some(PublishStatus {
            phase: PublishPhase::Succeeded,
            active: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let unpublish = UnpublishSync {
        status: Some(UnpublishStatus {
            phase: UnpublishPhase::Succeeded,
            request_id: Some("request-1".to_string()),
            extension_name: Some("inspecttask".to_string()),
            ..Default::default()
        }),
        should_requeue: false,
    };

    apply_unpublish_sync(&mut status, &unpublish);

    assert!(!status.publish.unwrap().active);
}

#[test]
fn delete_after_unpublish_requires_matching_succeeded_request_id() {
    let mut fe = sample_fe();
    fe.metadata.annotations = Some(BTreeMap::from([(
        ANNO_DELETE_AFTER_UNPUBLISH_REQUEST_ID.to_string(),
        "request-1".to_string(),
    )]));
    let matching = UnpublishSync {
        status: Some(UnpublishStatus {
            phase: UnpublishPhase::Succeeded,
            request_id: Some("request-1".to_string()),
            ..Default::default()
        }),
        should_requeue: false,
    };
    let mismatched = UnpublishSync {
        status: Some(UnpublishStatus {
            phase: UnpublishPhase::Succeeded,
            request_id: Some("request-2".to_string()),
            ..Default::default()
        }),
        should_requeue: false,
    };

    assert!(should_delete_after_unpublish(&fe, &matching));
    assert!(!should_delete_after_unpublish(&fe, &mismatched));
}

#[test]
fn publish_status_maps_failed_job_message() {
    let request = PublishRequest {
        request_id: "request-1".to_string(),
        artifact_digest: "sha256:artifact".to_string(),
        target_ref: NamespacedResourceRef {
            namespace: "extension-frontend-forge".to_string(),
            name: "ksbuilder-publish-config".to_string(),
            uid: None,
        },
        target_kind: "ConfigMap".to_string(),
    };
    let job = Job {
        metadata: ObjectMeta {
            name: Some("fe-inspecttask-publish-request".to_string()),
            namespace: Some("extension-frontend-forge".to_string()),
            uid: Some("job-uid".to_string()),
            ..Default::default()
        },
        status: Some(JobStatus {
            failed: Some(1),
            conditions: Some(vec![k8s_openapi::api::batch::v1::JobCondition {
                type_: "Failed".to_string(),
                status: "True".to_string(),
                message: Some("ksbuilder publish failed".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let status = publish_status_from_job(&request, &job);

    assert_eq!(status.phase, PublishPhase::Failed);
    assert_eq!(
        status.last_error.as_deref(),
        Some("ksbuilder publish failed")
    );
    assert_eq!(
        status.job_ref.unwrap().name,
        "fe-inspecttask-publish-request"
    );
}
