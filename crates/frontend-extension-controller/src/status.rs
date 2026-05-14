use super::*;

pub(crate) fn requeue_if_publish_or_unpublish_running(
    publish: &PublishSync,
    unpublish: &UnpublishSync,
    requeue_seconds: u64,
) -> Action {
    if publish.should_requeue || unpublish.should_requeue {
        Action::requeue(Duration::from_secs(requeue_seconds))
    } else {
        Action::await_change()
    }
}
pub(crate) fn current_or_existing_package_job(
    current_job: Option<&Job>,
    fe: &FrontendExtension,
) -> Option<PackageJobStatus> {
    current_job
        .map(package_job_status)
        .or_else(|| existing_package_job(fe))
}

pub(crate) fn packaging_fe_status(
    fe: &FrontendExtension,
    source_hash: &str,
    rebuild_token: &str,
    artifact_key: &str,
    job: &Job,
    message: &str,
) -> FrontendExtensionStatus {
    let generation = Some(fe.metadata.generation.unwrap_or_default());
    let publish = retained_publish_for_artifact_key(fe, artifact_key);
    FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Packaging,
        observed_generation: generation,
        observed_source_hash: Some(source_hash.to_string()),
        observed_rebuild_token: Some(rebuild_token.to_string()),
        artifact: None,
        download: Some(ExtensionDownloadStatus {
            ready: false,
            filename: String::new(),
            media_type: String::new(),
        }),
        package_job: Some(package_job_status(job)),
        publish: publish.clone(),
        unpublish: fe
            .status
            .as_ref()
            .and_then(|status| status.unpublish.clone()),
        conditions: vec![
            fe_condition("SourceValid", "True", "Validated", "", generation),
            fe_condition("ArtifactReady", "False", "Packaging", message, generation),
            fe_condition(
                "DownloadReady",
                "False",
                "ArtifactNotReady",
                message,
                generation,
            ),
            fe_publish_condition_from_status(publish.as_ref(), generation),
        ],
    }
}

pub(crate) fn ready_fe_status(
    fe: &FrontendExtension,
    source_hash: &str,
    rebuild_token: &str,
    artifact_key: &str,
    cm: &ConfigMap,
    metadata: PackageArtifactMetadata,
    package_job: Option<PackageJobStatus>,
) -> FrontendExtensionStatus {
    let generation = Some(fe.metadata.generation.unwrap_or_default());
    FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Ready,
        observed_generation: generation,
        observed_source_hash: Some(source_hash.to_string()),
        observed_rebuild_token: Some(rebuild_token.to_string()),
        artifact: Some(ExtensionArtifactStatus {
            storage: ArtifactStorageStatus {
                kind: ArtifactStorageKind::ConfigMap,
                ref_: namespaced_ref(cm),
                key: PACKAGE_KEY.to_string(),
            },
            digest: metadata.digest.clone(),
            size_bytes: i64::try_from(metadata.size_bytes).unwrap_or(i64::MAX),
            media_type: metadata.media_type.clone(),
            filename: metadata.filename.clone(),
            generated_at: metadata.generated_at,
            source_hash: metadata.source_hash,
            artifact_key: Some(artifact_key.to_string()),
        }),
        download: Some(ExtensionDownloadStatus {
            ready: true,
            filename: metadata.filename,
            media_type: metadata.media_type,
        }),
        package_job,
        publish: fe.status.as_ref().and_then(|status| status.publish.clone()),
        unpublish: fe
            .status
            .as_ref()
            .and_then(|status| status.unpublish.clone()),
        conditions: vec![
            fe_condition("SourceValid", "True", "Validated", "", generation),
            fe_condition("ArtifactReady", "True", "Generated", "", generation),
            fe_condition("DownloadReady", "True", "Available", "", generation),
            fe_publish_condition(fe, generation),
        ],
    }
}

pub(crate) fn failed_fe_status(
    fe: &FrontendExtension,
    source_hash: &str,
    rebuild_token: &str,
    artifact_key: &str,
    job: Option<&Job>,
    reason: &str,
    message: &str,
) -> FrontendExtensionStatus {
    let generation = Some(fe.metadata.generation.unwrap_or_default());
    let publish = retained_publish_for_artifact_key(fe, artifact_key);
    FrontendExtensionStatus {
        phase: FrontendExtensionPhase::Failed,
        observed_generation: generation,
        observed_source_hash: Some(source_hash.to_string()),
        observed_rebuild_token: Some(rebuild_token.to_string()),
        artifact: None,
        download: Some(ExtensionDownloadStatus {
            ready: false,
            filename: String::new(),
            media_type: String::new(),
        }),
        package_job: job.map(package_job_status),
        publish: publish.clone(),
        unpublish: fe
            .status
            .as_ref()
            .and_then(|status| status.unpublish.clone()),
        conditions: vec![
            fe_condition(
                "SourceValid",
                if reason == "InvalidSource" {
                    "False"
                } else {
                    "True"
                },
                reason,
                message,
                generation,
            ),
            fe_condition("ArtifactReady", "False", reason, message, generation),
            fe_condition(
                "DownloadReady",
                "False",
                "ArtifactNotReady",
                message,
                generation,
            ),
            fe_publish_condition_from_status(publish.as_ref(), generation),
        ],
    }
}

pub(crate) fn retained_publish_for_artifact_key(
    fe: &FrontendExtension,
    artifact_key: &str,
) -> Option<PublishStatus> {
    let status = fe.status.as_ref()?;
    if status
        .artifact
        .as_ref()
        .is_some_and(|artifact| artifact.artifact_key.as_deref() == Some(artifact_key))
    {
        status.publish.clone()
    } else {
        Some(PublishStatus::default())
    }
}

pub(crate) fn fe_condition(
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
    observed_generation: Option<i64>,
) -> ExtensionCondition {
    ExtensionCondition {
        type_: type_.to_string(),
        status: status.to_string(),
        reason: Some(reason.to_string()),
        message: if message.is_empty() {
            None
        } else {
            Some(message.to_string())
        },
        observed_generation,
        last_transition_time: None,
    }
}

pub(crate) fn fe_publish_condition(
    fe: &FrontendExtension,
    observed_generation: Option<i64>,
) -> ExtensionCondition {
    fe_publish_condition_from_status(
        fe.status
            .as_ref()
            .and_then(|status| status.publish.as_ref()),
        observed_generation,
    )
}

pub(crate) fn fe_publish_condition_from_status(
    publish: Option<&PublishStatus>,
    observed_generation: Option<i64>,
) -> ExtensionCondition {
    match publish {
        Some(publish) if matches!(publish.phase, PublishPhase::Succeeded) => fe_condition(
            "PublishSucceeded",
            "True",
            "Succeeded",
            "",
            observed_generation,
        ),
        Some(publish) if matches!(publish.phase, PublishPhase::Failed) => fe_condition(
            "PublishSucceeded",
            "False",
            "PublishFailed",
            publish.last_error.as_deref().unwrap_or(""),
            observed_generation,
        ),
        _ => fe_condition(
            "PublishSucceeded",
            "False",
            "NotRequested",
            "",
            observed_generation,
        ),
    }
}

pub(crate) fn namespaced_ref<K: ResourceExt>(obj: &K) -> NamespacedResourceRef {
    NamespacedResourceRef {
        namespace: obj.namespace().unwrap_or_default(),
        name: obj.name_any(),
        uid: obj.meta().uid.clone(),
    }
}

pub(crate) fn fe_status_needs_patch(
    fe: &FrontendExtension,
    desired_status: &FrontendExtensionStatus,
) -> bool {
    fe.status.as_ref() != Some(desired_status)
}

pub(crate) fn frontend_extension_status_labels(
    status: &FrontendExtensionStatus,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            LABEL_FE_PACKAGE_STATUS.to_string(),
            package_status_label_value(&status.phase).to_string(),
        ),
        (
            LABEL_FE_PUBLISH_STATUS.to_string(),
            publish_status_label_value(status.publish.as_ref()).to_string(),
        ),
    ])
}

pub(crate) fn package_status_label_value(phase: &FrontendExtensionPhase) -> &'static str {
    match phase {
        FrontendExtensionPhase::Ready => FE_PACKAGE_STATUS_READY,
        FrontendExtensionPhase::Failed => FE_PACKAGE_STATUS_FAILED,
        FrontendExtensionPhase::Pending | FrontendExtensionPhase::Packaging => {
            FE_PACKAGE_STATUS_PACKAGING
        }
    }
}

pub(crate) fn publish_status_label_value(publish: Option<&PublishStatus>) -> &'static str {
    match publish {
        Some(status) if matches!(status.phase, PublishPhase::Pending | PublishPhase::Running) => {
            FE_PUBLISH_STATUS_PUBLISHING
        }
        Some(status) if matches!(status.phase, PublishPhase::Succeeded) && status.active => {
            FE_PUBLISH_STATUS_PUBLISHED
        }
        Some(status) if matches!(status.phase, PublishPhase::Failed) => FE_PUBLISH_STATUS_FAILED,
        _ => FE_PUBLISH_STATUS_NOT_PUBLISHED,
    }
}

pub(crate) fn fe_status_labels_need_patch(
    fe: &FrontendExtension,
    desired_status: &FrontendExtensionStatus,
) -> bool {
    let labels = frontend_extension_status_labels(desired_status);
    labels.iter().any(|(key, value)| {
        fe.metadata
            .labels
            .as_ref()
            .and_then(|current| current.get(key))
            != Some(value)
    }) || fe.metadata.labels.as_ref().is_some_and(|current| {
        current.contains_key(DEPRECATED_LABEL_FE_PACKAGE_STATUS)
            || current.contains_key(DEPRECATED_LABEL_FE_PUBLISH_STATUS)
    })
}

pub(crate) async fn patch_fe_status(
    fe_api: &Api<FrontendExtension>,
    fe: &FrontendExtension,
    status: FrontendExtensionStatus,
) -> Result<(), Error> {
    let fe_name = fe.name_any();
    if fe_status_needs_patch(fe, &status) {
        let patch = frontend_extension_status_patch(&status, &fe_name)?;

        fe_api
            .patch_status(&fe_name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .with_context(|_| PatchFrontendExtensionStatusSnafu {
                name: fe_name.clone(),
            })?;
    }

    patch_fe_status_labels(fe_api, fe, &status, &fe_name).await?;

    Ok(())
}

pub(crate) async fn patch_fe_status_labels(
    fe_api: &Api<FrontendExtension>,
    fe: &FrontendExtension,
    status: &FrontendExtensionStatus,
    fe_name: &str,
) -> Result<(), Error> {
    if !fe_status_labels_need_patch(fe, status) {
        return Ok(());
    }

    let patch = frontend_extension_status_labels_patch(status);
    fe_api
        .patch(fe_name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .with_context(|_| PatchFrontendExtensionStatusLabelsSnafu {
            name: fe_name.to_string(),
        })?;

    Ok(())
}

pub(crate) fn frontend_extension_status_labels_patch(
    status: &FrontendExtensionStatus,
) -> serde_json::Value {
    let mut labels = serde_json::Map::new();
    for (key, value) in frontend_extension_status_labels(status) {
        labels.insert(key, json!(value));
    }
    labels.insert(
        DEPRECATED_LABEL_FE_PACKAGE_STATUS.to_string(),
        serde_json::Value::Null,
    );
    labels.insert(
        DEPRECATED_LABEL_FE_PUBLISH_STATUS.to_string(),
        serde_json::Value::Null,
    );

    json!({
        "metadata": {
            "labels": labels,
        },
    })
}

pub(crate) fn frontend_extension_status_patch(
    status: &FrontendExtensionStatus,
    name: &str,
) -> Result<serde_json::Value, Error> {
    let mut status_value = serde_json::to_value(status).with_context(|_| {
        SerializeFrontendExtensionStatusPatchSnafu {
            name: name.to_string(),
        }
    })?;
    let status_object = status_value.as_object_mut().ok_or_else(|| {
        Error::InvalidFrontendExtensionStatusPatchShape {
            name: name.to_string(),
        }
    })?;

    if status.artifact.is_none() {
        status_object.insert("artifact".to_string(), serde_json::Value::Null);
    }
    if status.download.is_none() {
        status_object.insert("download".to_string(), serde_json::Value::Null);
    }
    if status.package_job.is_none() {
        status_object.insert("packageJob".to_string(), serde_json::Value::Null);
    } else if status
        .package_job
        .as_ref()
        .is_some_and(|package_job| package_job.message.is_none())
        && let Some(package_job) = status_object
            .get_mut("packageJob")
            .and_then(serde_json::Value::as_object_mut)
    {
        package_job.insert("message".to_string(), serde_json::Value::Null);
    }
    if status.publish.is_none() {
        status_object.insert("publish".to_string(), serde_json::Value::Null);
    } else if status
        .publish
        .as_ref()
        .is_some_and(|publish| publish.last_error.is_none())
        && let Some(publish) = status_object
            .get_mut("publish")
            .and_then(serde_json::Value::as_object_mut)
    {
        publish.insert("lastError".to_string(), serde_json::Value::Null);
    }
    if status.unpublish.is_none() {
        status_object.insert("unpublish".to_string(), serde_json::Value::Null);
    } else if status
        .unpublish
        .as_ref()
        .is_some_and(|unpublish| unpublish.last_error.is_none())
        && let Some(unpublish) = status_object
            .get_mut("unpublish")
            .and_then(serde_json::Value::as_object_mut)
    {
        unpublish.insert("lastError".to_string(), serde_json::Value::Null);
    }

    Ok(json!({
        "status": status_value,
    }))
}
