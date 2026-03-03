use chrono::Utc;
use frontend_forge_api::{
    FrontendIntegration, FrontendIntegrationPhase, FrontendIntegrationStatus, JSBundle,
    LastBuildStatus, ResourceRef,
};
use frontend_forge_common::{
    ANNO_MANIFEST_HASH, ANNO_OBSERVED_GENERATION, BUILD_KIND_VALUE, CommonError, LABEL_BUILD_KIND,
    LABEL_ENABLED, LABEL_FI_NAME, LABEL_MANAGED_BY, LABEL_MANIFEST_HASH, LABEL_SPEC_HASH,
    MANAGED_BY_VALUE, default_bundle_name, hash_label_value, job_name, serializable_hash,
};
use futures::StreamExt;
use k8s_openapi::api::batch::v1::JobStatus;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use kube::{Api, Client, Resource, ResourceExt};
use kube_runtime::controller::{Action, Controller};
use kube_runtime::watcher;
use serde_json::json;
use snafu::{ResultExt, Snafu};
use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("spec/hash error: {source}"))]
    Common { source: CommonError },
    #[snafu(display("failed to initialize Kubernetes client: {source}"))]
    KubeClientInit { source: kube::Error },
    #[snafu(display("failed to patch FrontendIntegration status {namespace}/{name}: {source}"))]
    PatchFrontendIntegrationStatus {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to patch FrontendIntegration metadata {namespace}/{name}: {source}"))]
    PatchFrontendIntegrationMetadata {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display(
        "failed to serialize FrontendIntegration status patch for {namespace}/{name}: {source}"
    ))]
    SerializeFrontendIntegrationStatusPatch {
        namespace: String,
        name: String,
        source: serde_json::Error,
    },
    #[snafu(display(
        "serialized FrontendIntegration status patch for {namespace}/{name} was not a JSON object"
    ))]
    InvalidFrontendIntegrationStatusPatchShape { namespace: String, name: String },
    #[snafu(display(
        "failed to list Jobs in {namespace} for FrontendIntegration {fi_name} and specHash {spec_hash}: {source}"
    ))]
    ListJobsForHash {
        namespace: String,
        fi_name: String,
        spec_hash: String,
        source: kube::Error,
    },
    #[snafu(display("failed to get JSBundle {namespace}/{name}: {source}"))]
    GetJsBundle {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to patch JSBundle {namespace}/{name}: {source}"))]
    PatchJsBundle {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to create Job {namespace}/{name}: {source}"))]
    CreateJob {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to get existing Job after conflict {namespace}/{name}: {source}"))]
    GetJobAfterConflict {
        namespace: String,
        name: String,
        source: kube::Error,
    },
}

#[derive(Clone, Debug)]
struct ControllerConfig {
    work_namespace: String,
    runner_image: String,
    runner_service_account: Option<String>,
    build_service_base_url: String,
    jsbundle_configmap_namespace: String,
    jsbundle_config_key: String,
    build_service_timeout_seconds: u64,
    stale_check_grace_seconds: u64,
    reconcile_requeue_seconds: u64,
    job_active_deadline_seconds: i64,
    job_ttl_seconds_after_finished: Option<i32>,
}

impl ControllerConfig {
    fn from_env() -> Self {
        Self {
            work_namespace: env::var("WORK_NAMESPACE")
                .unwrap_or_else(|_| "extension-frontend-forge".to_string()),
            runner_image: env::var("RUNNER_IMAGE")
                .unwrap_or_else(|_| "spike2044/frontend-forge-runner:latest".to_string()),
            runner_service_account: env::var("RUNNER_SERVICE_ACCOUNT").ok(),
            build_service_base_url: env::var("BUILD_SERVICE_BASE_URL").unwrap_or_else(|_| {
                "http://frontend-forge.extension-frontend-forge.svc".to_string()
            }),
            jsbundle_configmap_namespace: env::var("JSBUNDLE_CONFIGMAP_NAMESPACE")
                .unwrap_or_else(|_| "extension-frontend-forge".to_string()),
            jsbundle_config_key: env::var("JSBUNDLE_CONFIG_KEY")
                .unwrap_or_else(|_| "index.js".to_string()),
            build_service_timeout_seconds: env::var("BUILD_SERVICE_TIMEOUT_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            stale_check_grace_seconds: env::var("STALE_CHECK_GRACE_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            reconcile_requeue_seconds: env::var("RECONCILE_REQUEUE_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            job_active_deadline_seconds: env::var("JOB_ACTIVE_DEADLINE_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            job_ttl_seconds_after_finished: env::var("JOB_TTL_SECONDS_AFTER_FINISHED")
                .ok()
                .and_then(|v| v.parse().ok())
                .or(Some(DEFAULT_JOB_TTL_SECONDS_AFTER_FINISHED)),
        }
    }
}

#[derive(Clone)]
struct ContextData {
    client: Client,
    config: ControllerConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObservedJobPhase {
    Pending,
    Running,
    Succeeded,
    Failed,
}

const JSBUNDLE_STATE_AVAILABLE: &str = "Available";
const JSBUNDLE_STATE_DISABLED: &str = "Disabled";
const DEFAULT_JOB_TTL_SECONDS_AFTER_FINISHED: i32 = 60 * 60;

fn build_spec_hash(fi: &FrontendIntegration) -> Result<String, CommonError> {
    serializable_hash(&fi.spec.without_enabled())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,frontend_forge_controller=debug".into()),
        )
        .init();

    let client = Client::try_default().await.context(KubeClientInitSnafu)?;
    let ctx = Arc::new(ContextData {
        client: client.clone(),
        config: ControllerConfig::from_env(),
    });

    let fi_api = Api::<FrontendIntegration>::all(client.clone());
    let job_api = Api::<Job>::all(client.clone());
    Controller::new(fi_api, watcher::Config::default())
        .owns(job_api, watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|result| async move {
            match result {
                Ok((obj_ref, action)) => info!(?obj_ref, ?action, "reconciled"),
                Err(err) => error!(error = %err, "controller reconcile stream error"),
            }
        })
        .await;

    info!("controller shutdown complete");

    Ok(())
}

fn error_policy(_fi: Arc<FrontendIntegration>, err: &Error, _ctx: Arc<ContextData>) -> Action {
    warn!(error = %err, "reconcile failed; requeueing");
    Action::requeue(Duration::from_secs(10))
}

async fn reconcile(fi: Arc<FrontendIntegration>, ctx: Arc<ContextData>) -> Result<Action, Error> {
    let fi_name = fi.name_any();
    let client = ctx.client.clone();
    let work_ns = ctx.config.work_namespace.clone();

    let fi_api = Api::<FrontendIntegration>::all(client.clone());
    let job_api = Api::<Job>::namespaced(client.clone(), &work_ns);
    let bundle_api = Api::<JSBundle>::all(client.clone());

    if fi.meta().deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }

    patch_fi_enabled_label_if_needed(&fi_api, &fi).await?;

    let spec_hash = build_spec_hash(&fi).context(CommonSnafu)?;
    info!(
        fi = %fi_name,
        spec_hash,
        phase = ?fi.status.as_ref().map(|s| &s.phase),
        "reconcile started"
    );
    let desired_bundle_name = default_bundle_name(&fi_name);

    let current_bundle = get_bundle_opt(&bundle_api, &desired_bundle_name).await?;

    if !fi.spec.enabled() {
        if let Some(bundle) = current_bundle.as_ref() {
            sync_jsbundle_enabled_state(&bundle_api, &fi, bundle, false).await?;
        }
        patch_fi_status(&fi_api, &fi, disabled_status(&fi, current_bundle.as_ref())).await?;
        return Ok(Action::await_change());
    }

    let needs_build = needs_new_build(&fi, &spec_hash, current_bundle.as_ref());
    if needs_build {
        let existing_job = find_job_for_hash(&job_api, &work_ns, &fi_name, &spec_hash).await?;
        let chosen_job = if let Some(job) = existing_job
            .filter(|j| should_reuse_build_job(&fi, j, current_bundle.as_ref(), &spec_hash))
        {
            job
        } else {
            let job_name = job_name(&fi_name, &spec_hash);
            let desired_job = make_build_job(
                &fi,
                &ctx.config,
                &job_name,
                &desired_bundle_name,
                &spec_hash,
            );
            create_or_get_job(&job_api, &work_ns, desired_job, &job_name).await?
        };

        let status = building_status(
            &fi,
            &spec_hash,
            &desired_bundle_name,
            &chosen_job,
            "Build job scheduled",
        );
        patch_fi_status(&fi_api, &fi, status).await?;
        return Ok(Action::requeue(Duration::from_secs(
            ctx.config.reconcile_requeue_seconds,
        )));
    }

    let action = sync_status_from_children(
        &fi,
        &fi_api,
        &job_api,
        &bundle_api,
        &work_ns,
        &desired_bundle_name,
        &spec_hash,
        ctx.config.reconcile_requeue_seconds,
    )
    .await?;

    Ok(action)
}

fn needs_new_build(fi: &FrontendIntegration, spec_hash: &str, bundle: Option<&JSBundle>) -> bool {
    let status = fi.status.as_ref();
    let observed_hash = status
        .and_then(|s| s.observed_spec_hash.as_deref())
        .or_else(|| status.and_then(|s| s.observed_manifest_hash.as_deref()));
    let phase = status.map(|s| s.phase.clone());

    let hash_changed = observed_hash != Some(spec_hash);
    let pending_initial = status.is_none();
    let missing_matching_bundle = observed_hash == Some(spec_hash)
        && !matches!(
            phase,
            Some(FrontendIntegrationPhase::Building | FrontendIntegrationPhase::Failed)
        )
        && !bundle
            .map(|bundle| bundle_matches_spec_hash(bundle, spec_hash))
            .unwrap_or(false);

    hash_changed || pending_initial || missing_matching_bundle
}

fn should_reuse_build_job(
    fi: &FrontendIntegration,
    job: &Job,
    bundle: Option<&JSBundle>,
    spec_hash: &str,
) -> bool {
    match observed_job_phase(job.status.as_ref()) {
        ObservedJobPhase::Pending | ObservedJobPhase::Running => true,
        ObservedJobPhase::Succeeded => {
            let bundle_ready = bundle
                .map(|bundle| bundle_matches_spec_hash(bundle, spec_hash))
                .unwrap_or(false);
            bundle_ready
                && !matches!(
                    fi.status.as_ref().map(|s| s.phase.clone()),
                    Some(FrontendIntegrationPhase::Failed)
                )
        }
        ObservedJobPhase::Failed => false,
    }
}

async fn sync_status_from_children(
    fi: &FrontendIntegration,
    fi_api: &Api<FrontendIntegration>,
    job_api: &Api<Job>,
    bundle_api: &Api<JSBundle>,
    namespace: &str,
    bundle_name: &str,
    spec_hash: &str,
    requeue_seconds: u64,
) -> Result<Action, Error> {
    let fi_name = fi.name_any();
    let current_job = find_job_for_hash(job_api, namespace, &fi_name, spec_hash).await?;

    if let Some(job) = current_job {
        match observed_job_phase(job.status.as_ref()) {
            ObservedJobPhase::Pending | ObservedJobPhase::Running => {
                let status = building_status(fi, spec_hash, bundle_name, &job, "Build in progress");
                patch_fi_status(fi_api, fi, status).await?;
                return Ok(Action::requeue(Duration::from_secs(requeue_seconds)));
            }
            ObservedJobPhase::Failed => {
                let msg =
                    extract_job_message(&job).unwrap_or_else(|| "Build job failed".to_string());
                let status = failed_status(fi, spec_hash, msg);
                patch_fi_status(fi_api, fi, status).await?;
                return Ok(Action::await_change());
            }
            ObservedJobPhase::Succeeded => {
                let bundle = get_bundle_opt(bundle_api, bundle_name).await?;
                if let Some(bundle) = bundle {
                    if bundle_matches_spec_hash(&bundle, spec_hash) {
                        sync_jsbundle_enabled_state(bundle_api, fi, &bundle, true).await?;
                        let status = succeeded_status(fi, spec_hash, &bundle, &job);
                        patch_fi_status(fi_api, fi, status).await?;
                        return Ok(Action::await_change());
                    }
                    let status = building_status(
                        fi,
                        spec_hash,
                        bundle_name,
                        &job,
                        "Job succeeded; waiting for JSBundle with matching spec-hash",
                    );
                    patch_fi_status(fi_api, fi, status).await?;
                    return Ok(Action::requeue(Duration::from_secs(requeue_seconds)));
                }

                let status = building_status(
                    fi,
                    spec_hash,
                    bundle_name,
                    &job,
                    "Job succeeded; waiting for JSBundle materialization",
                );
                patch_fi_status(fi_api, fi, status).await?;
                return Ok(Action::requeue(Duration::from_secs(requeue_seconds)));
            }
        }
    }

    if let Some(bundle) = get_bundle_opt(bundle_api, bundle_name).await? {
        if bundle_matches_spec_hash(&bundle, spec_hash) {
            sync_jsbundle_enabled_state(bundle_api, fi, &bundle, true).await?;
            let status = FrontendIntegrationStatus {
                phase: FrontendIntegrationPhase::Succeeded,
                observed_spec_hash: Some(spec_hash.to_string()),
                observed_manifest_hash: bundle_manifest_hash(&bundle),
                observed_generation: Some(fi.metadata.generation.unwrap_or_default()),
                last_build: fi.status.as_ref().and_then(|s| s.last_build.clone()),
                bundle_ref: Some(resource_ref(&bundle)),
                message: Some("JSBundle ready".to_string()),
                conditions: vec![],
            };
            patch_fi_status(fi_api, fi, status).await?;
        }
    }

    Ok(Action::await_change())
}

async fn find_job_for_hash(
    job_api: &Api<Job>,
    namespace: &str,
    fi_name: &str,
    spec_hash: &str,
) -> Result<Option<Job>, Error> {
    let selector = format!(
        "{}={},{}={}",
        LABEL_FI_NAME,
        fi_name,
        LABEL_SPEC_HASH,
        hash_label_value(spec_hash)
    );
    let jobs = job_api
        .list(&ListParams::default().labels(&selector))
        .await
        .with_context(|_| ListJobsForHashSnafu {
            namespace: namespace.to_string(),
            fi_name: fi_name.to_string(),
            spec_hash: spec_hash.to_string(),
        })?;
    let mut items = jobs.items;
    items.sort_by_key(|j| j.metadata.creation_timestamp.clone());
    let latest_job = items.pop();
    if items.len() > 0 {
        if let Some(job) = latest_job.as_ref() {
            let job_name = job.name_any();
            warn!(
                fi = %fi_name,
                job = %job_name,
                "multiple jobs found for same spec_hash, using latest"
            );
        }
    }
    Ok(latest_job)
}

fn observed_job_phase(status: Option<&JobStatus>) -> ObservedJobPhase {
    let Some(status) = status else {
        return ObservedJobPhase::Pending;
    };

    if status.failed.unwrap_or(0) > 0 {
        return ObservedJobPhase::Failed;
    }
    if status.succeeded.unwrap_or(0) > 0 {
        return ObservedJobPhase::Succeeded;
    }
    if status.active.unwrap_or(0) > 0 {
        return ObservedJobPhase::Running;
    }

    if let Some(conditions) = &status.conditions {
        for cond in conditions {
            if cond.status != "True" {
                continue;
            }
            if cond.type_ == "Failed" {
                return ObservedJobPhase::Failed;
            }
            if cond.type_ == "Complete" {
                return ObservedJobPhase::Succeeded;
            }
        }
    }

    ObservedJobPhase::Pending
}

fn extract_job_message(job: &Job) -> Option<String> {
    let status = job.status.as_ref()?;
    if let Some(conditions) = &status.conditions {
        if let Some(cond) = conditions
            .iter()
            .find(|c| c.status == "True" && c.type_ == "Failed")
        {
            return cond.message.clone().or_else(|| cond.reason.clone());
        }
    }
    None
}

fn bundle_matches_spec_hash(bundle: &JSBundle, spec_hash: &str) -> bool {
    let expected = hash_label_value(spec_hash);
    bundle
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(LABEL_SPEC_HASH))
        .map(|v| v == &expected)
        .unwrap_or(false)
}

fn labels_for(fi_name: &str, spec_hash: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_MANAGED_BY.to_string(), MANAGED_BY_VALUE.to_string()),
        (LABEL_FI_NAME.to_string(), fi_name.to_string()),
        (LABEL_SPEC_HASH.to_string(), hash_label_value(spec_hash)),
    ])
}

fn base_owner_ref<T>(obj: &T) -> Option<OwnerReference>
where
    T: Resource<DynamicType = ()>,
{
    obj.controller_owner_ref(&())
}

fn make_build_job(
    fi: &FrontendIntegration,
    config: &ControllerConfig,
    job_name: &str,
    jsbundle_name: &str,
    spec_hash: &str,
) -> Job {
    let fi_name = fi.name_any();
    let mut labels = labels_for(&fi_name, spec_hash);
    labels.insert(LABEL_BUILD_KIND.to_string(), BUILD_KIND_VALUE.to_string());

    let mut annotations = BTreeMap::new();
    if let Some(generation) = fi.metadata.generation {
        annotations.insert(ANNO_OBSERVED_GENERATION.to_string(), generation.to_string());
    }

    let env = vec![
        EnvVar {
            name: "FI_NAME".to_string(),
            value: Some(fi_name.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "SPEC_HASH".to_string(),
            value: Some(spec_hash.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "JSBUNDLE_NAME".to_string(),
            value: Some(jsbundle_name.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "BUILD_SERVICE_BASE_URL".to_string(),
            value: Some(config.build_service_base_url.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "JSBUNDLE_CONFIGMAP_NAMESPACE".to_string(),
            value: Some(config.jsbundle_configmap_namespace.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "JSBUNDLE_CONFIG_KEY".to_string(),
            value: Some(config.jsbundle_config_key.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "BUILD_SERVICE_TIMEOUT_SECONDS".to_string(),
            value: Some(config.build_service_timeout_seconds.to_string()),
            ..Default::default()
        },
        EnvVar {
            name: "STALE_CHECK_GRACE_SECONDS".to_string(),
            value: Some(config.stale_check_grace_seconds.to_string()),
            ..Default::default()
        },
    ];

    let container = Container {
        name: "runner".to_string(),
        image: Some(config.runner_image.clone()),
        env: Some(env),
        ..Default::default()
    };

    Job {
        metadata: ObjectMeta {
            name: Some(job_name.to_string()),
            namespace: Some(config.work_namespace.clone()),
            labels: Some(labels),
            annotations: Some(annotations),
            owner_references: base_owner_ref(fi).map(|o| vec![o]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            active_deadline_seconds: Some(config.job_active_deadline_seconds),
            ttl_seconds_after_finished: config.job_ttl_seconds_after_finished,
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(BTreeMap::from([(
                        "app.kubernetes.io/name".to_string(),
                        "frontend-forge-runner".to_string(),
                    )])),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    service_account_name: config.runner_service_account.clone(),
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            backoff_limit: Some(0),
            ..Default::default()
        }),
        status: None,
    }
}

async fn create_or_get_job(
    job_api: &Api<Job>,
    namespace: &str,
    job: Job,
    name: &str,
) -> Result<Job, Error> {
    match job_api.create(&PostParams::default(), &job).await {
        Ok(created) => Ok(created),
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            Ok(job_api
                .get(name)
                .await
                .with_context(|_| GetJobAfterConflictSnafu {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                })?)
        }
        Err(err) => Err(Error::CreateJob {
            namespace: namespace.to_string(),
            name: name.to_string(),
            source: err,
        }),
    }
}

async fn get_bundle_opt(bundle_api: &Api<JSBundle>, name: &str) -> Result<Option<JSBundle>, Error> {
    bundle_api
        .get_opt(name)
        .await
        .with_context(|_| GetJsBundleSnafu {
            namespace: "<cluster>".to_string(),
            name: name.to_string(),
        })
}

fn enabled_label_value(enabled: bool) -> &'static str {
    if enabled { "true" } else { "false" }
}

async fn patch_fi_enabled_label_if_needed(
    fi_api: &Api<FrontendIntegration>,
    fi: &FrontendIntegration,
) -> Result<(), Error> {
    let desired = enabled_label_value(fi.spec.enabled());
    let current = fi
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(LABEL_ENABLED))
        .map(String::as_str);
    if current == Some(desired) {
        return Ok(());
    }

    let fi_name = fi.name_any();
    let namespace = fi.namespace().unwrap_or_else(|| "<cluster>".to_string());
    let patch = json!({
        "metadata": {
            "labels": {
                LABEL_ENABLED: desired,
            }
        }
    });
    fi_api
        .patch(&fi_name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .with_context(|_| PatchFrontendIntegrationMetadataSnafu {
            namespace,
            name: fi_name.clone(),
        })?;
    Ok(())
}

async fn sync_jsbundle_enabled_state(
    bundle_api: &Api<JSBundle>,
    fi: &FrontendIntegration,
    bundle: &JSBundle,
    enabled: bool,
) -> Result<(), Error> {
    patch_jsbundle_owner_ref_if_needed(bundle_api, fi, bundle).await?;
    patch_jsbundle_enabled_label_if_needed(bundle_api, bundle, enabled).await?;
    let desired_state = if enabled {
        JSBUNDLE_STATE_AVAILABLE
    } else {
        JSBUNDLE_STATE_DISABLED
    };
    patch_jsbundle_state_if_needed(bundle_api, bundle, desired_state).await?;
    Ok(())
}

async fn patch_jsbundle_owner_ref_if_needed(
    bundle_api: &Api<JSBundle>,
    fi: &FrontendIntegration,
    bundle: &JSBundle,
) -> Result<(), Error> {
    let Some(owner_ref) = base_owner_ref(fi) else {
        return Ok(());
    };

    let mut owners = bundle.metadata.owner_references.clone().unwrap_or_default();
    if owners.iter().any(|owner| owner.uid == owner_ref.uid) {
        return Ok(());
    }
    owners.push(owner_ref);

    let name = bundle.name_any();
    let patch = json!({
        "metadata": {
            "ownerReferences": owners,
        }
    });
    match bundle_api
        .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(source) => Err(Error::PatchJsBundle {
            namespace: "<cluster>".to_string(),
            name,
            source,
        }),
    }
}

async fn patch_jsbundle_enabled_label_if_needed(
    bundle_api: &Api<JSBundle>,
    bundle: &JSBundle,
    enabled: bool,
) -> Result<(), Error> {
    let desired = enabled_label_value(enabled);
    let current = bundle
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(LABEL_ENABLED))
        .map(String::as_str);
    if current == Some(desired) {
        return Ok(());
    }

    let name = bundle.name_any();
    let patch = json!({
        "metadata": {
            "labels": {
                LABEL_ENABLED: desired,
            }
        }
    });
    match bundle_api
        .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(source) => Err(Error::PatchJsBundle {
            namespace: "<cluster>".to_string(),
            name,
            source,
        }),
    }
}

async fn patch_jsbundle_state_if_needed(
    bundle_api: &Api<JSBundle>,
    bundle: &JSBundle,
    desired_state: &str,
) -> Result<(), Error> {
    let current = bundle
        .status
        .as_ref()
        .and_then(|status| status.state.as_deref());
    if current == Some(desired_state) {
        return Ok(());
    }

    let name = bundle.name_any();
    let patch = json!({
        "status": {
            "state": desired_state,
        }
    });
    match bundle_api
        .patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            match bundle_api
                .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
                .await
            {
                Ok(_) => Ok(()),
                Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
                Err(source) => Err(Error::PatchJsBundle {
                    namespace: "<cluster>".to_string(),
                    name,
                    source,
                }),
            }
        }
        Err(source) => Err(Error::PatchJsBundle {
            namespace: "<cluster>".to_string(),
            name,
            source,
        }),
    }
}

fn resource_ref<K: ResourceExt>(obj: &K) -> ResourceRef {
    ResourceRef {
        name: obj.name_any(),
        namespace: obj.namespace(),
        uid: obj.meta().uid.clone(),
    }
}

fn disabled_status(
    fi: &FrontendIntegration,
    bundle: Option<&JSBundle>,
) -> FrontendIntegrationStatus {
    FrontendIntegrationStatus {
        phase: FrontendIntegrationPhase::Pending,
        observed_spec_hash: fi
            .status
            .as_ref()
            .and_then(|s| s.observed_spec_hash.clone()),
        observed_manifest_hash: fi
            .status
            .as_ref()
            .and_then(|s| s.observed_manifest_hash.clone()),
        observed_generation: Some(fi.metadata.generation.unwrap_or_default()),
        last_build: None,
        bundle_ref: bundle.map(resource_ref),
        message: Some("Disabled".to_string()),
        conditions: vec![],
    }
}

fn building_status(
    fi: &FrontendIntegration,
    spec_hash: &str,
    bundle_name: &str,
    job: &Job,
    message: &str,
) -> FrontendIntegrationStatus {
    FrontendIntegrationStatus {
        phase: FrontendIntegrationPhase::Building,
        observed_spec_hash: Some(spec_hash.to_string()),
        observed_manifest_hash: fi
            .status
            .as_ref()
            .and_then(|s| s.observed_manifest_hash.clone()),
        observed_generation: Some(fi.metadata.generation.unwrap_or_default()),
        last_build: Some(LastBuildStatus {
            job_ref: Some(resource_ref(job)),
            started_at: Some(Utc::now()),
        }),
        bundle_ref: Some(ResourceRef {
            name: bundle_name.to_string(),
            namespace: None,
            uid: None,
        }),
        message: Some(message.to_string()),
        conditions: vec![],
    }
}

fn succeeded_status(
    fi: &FrontendIntegration,
    spec_hash: &str,
    bundle: &JSBundle,
    job: &Job,
) -> FrontendIntegrationStatus {
    FrontendIntegrationStatus {
        phase: FrontendIntegrationPhase::Succeeded,
        observed_spec_hash: Some(spec_hash.to_string()),
        observed_manifest_hash: bundle_manifest_hash(bundle),
        observed_generation: Some(fi.metadata.generation.unwrap_or_default()),
        last_build: Some(LastBuildStatus {
            job_ref: Some(resource_ref(job)),
            started_at: fi
                .status
                .as_ref()
                .and_then(|s| s.last_build.clone())
                .and_then(|b| b.started_at),
        }),
        bundle_ref: Some(resource_ref(bundle)),
        message: Some("Build succeeded".to_string()),
        conditions: vec![],
    }
}

fn bundle_manifest_hash(bundle: &JSBundle) -> Option<String> {
    if let Some(v) = bundle
        .metadata
        .annotations
        .as_ref()
        .and_then(|annos| annos.get(ANNO_MANIFEST_HASH))
        .cloned()
    {
        return Some(v);
    }

    bundle
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(LABEL_MANIFEST_HASH))
        .map(|v| {
            if v.starts_with("sha256:") {
                v.clone()
            } else {
                format!("sha256:{}", v)
            }
        })
}

fn failed_status(
    fi: &FrontendIntegration,
    spec_hash: &str,
    message: String,
) -> FrontendIntegrationStatus {
    FrontendIntegrationStatus {
        phase: FrontendIntegrationPhase::Failed,
        observed_spec_hash: Some(spec_hash.to_string()),
        observed_manifest_hash: fi
            .status
            .as_ref()
            .and_then(|s| s.observed_manifest_hash.clone()),
        observed_generation: Some(fi.metadata.generation.unwrap_or_default()),
        last_build: fi.status.as_ref().and_then(|s| s.last_build.clone()),
        bundle_ref: fi.status.as_ref().and_then(|s| s.bundle_ref.clone()),
        message: Some(message),
        conditions: vec![],
    }
}

async fn patch_fi_status(
    fi_api: &Api<FrontendIntegration>,
    fi: &FrontendIntegration,
    status: FrontendIntegrationStatus,
) -> Result<(), Error> {
    let fi_name = fi.name_any();
    let namespace = fi.namespace().unwrap_or_else(|| "<cluster>".to_string());
    let patch = frontend_integration_status_patch(&status, &namespace, &fi_name)?;

    fi_api
        .patch_status(&fi_name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .with_context(|_| PatchFrontendIntegrationStatusSnafu {
            namespace,
            name: fi_name.clone(),
        })?;

    Ok(())
}

fn frontend_integration_status_patch(
    status: &FrontendIntegrationStatus,
    namespace: &str,
    name: &str,
) -> Result<serde_json::Value, Error> {
    let mut status_value = serde_json::to_value(status).with_context(|_| {
        SerializeFrontendIntegrationStatusPatchSnafu {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    })?;
    let status_object = status_value.as_object_mut().ok_or_else(|| {
        Error::InvalidFrontendIntegrationStatusPatchShape {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    })?;

    if status.last_build.is_none() {
        status_object.insert("last_build".to_string(), serde_json::Value::Null);
    }
    if status.bundle_ref.is_none() {
        status_object.insert("bundle_ref".to_string(), serde_json::Value::Null);
    }

    Ok(json!({
        "status": status_value,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontend_forge_api::{
        FrontendIntegrationSpec, IframePageSpec, MenuNodeType, MenuPlacement, PageSpec, PageType,
        PrimaryMenuSpec,
    };
    use k8s_openapi::api::batch::v1::JobStatus;
    use kube::core::ObjectMeta;

    fn fi(name: &str, status: Option<FrontendIntegrationStatus>) -> FrontendIntegration {
        FrontendIntegration {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                generation: Some(3),
                ..Default::default()
            },
            spec: FrontendIntegrationSpec {
                display_name: None,
                enabled: Some(true),
                menus: vec![PrimaryMenuSpec {
                    display_name: "demo".to_string(),
                    key: "demo".to_string(),
                    placement: MenuPlacement::Global,
                    type_: MenuNodeType::Page,
                    children: vec![],
                }],
                pages: vec![PageSpec {
                    key: "demo".to_string(),
                    type_: PageType::Iframe,
                    crd_table: None,
                    iframe: Some(IframePageSpec {
                        src: "http://example.test".to_string(),
                    }),
                }],
                builder: None,
            },
            status,
        }
    }

    fn spec_hash(fi: &FrontendIntegration) -> Result<String, CommonError> {
        build_spec_hash(fi)
    }

    fn bundle_for_hash(name: &str, spec_hash: &str) -> JSBundle {
        JSBundle {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                labels: Some(BTreeMap::from([(
                    LABEL_SPEC_HASH.to_string(),
                    hash_label_value(spec_hash),
                )])),
                ..Default::default()
            },
            spec: frontend_forge_api::JsBundleSpec {
                raw: None,
                raw_from: None,
            },
            status: None,
        }
    }

    #[test]
    fn build_hash_ignores_enabled() -> Result<(), CommonError> {
        let mut enabled_fi = fi("demo", None);
        enabled_fi.spec.enabled = Some(true);

        let mut disabled_fi = fi("demo", None);
        disabled_fi.spec.enabled = Some(false);

        assert_eq!(spec_hash(&enabled_fi)?, spec_hash(&disabled_fi)?);
        Ok(())
    }

    #[test]
    fn needs_build_when_hash_changes() {
        let mut fi = fi(
            "demo",
            Some(FrontendIntegrationStatus {
                observed_spec_hash: Some("sha256:old".to_string()),
                phase: FrontendIntegrationPhase::Succeeded,
                ..Default::default()
            }),
        );
        fi.spec.enabled = Some(true);
        assert!(needs_new_build(&fi, "sha256:new", None));
    }

    #[test]
    fn does_not_build_when_observed_hash_matches() -> Result<(), CommonError> {
        let mut fi = fi("demo", None);
        fi.spec.enabled = Some(true);
        let hash = spec_hash(&fi)?;
        let bundle = bundle_for_hash("fi-demo", &hash);
        fi.status = Some(FrontendIntegrationStatus {
            observed_spec_hash: Some(hash.clone()),
            phase: FrontendIntegrationPhase::Succeeded,
            ..Default::default()
        });

        assert!(!needs_new_build(&fi, &hash, Some(&bundle)));
        Ok(())
    }

    #[test]
    fn does_not_auto_retry_failed_build_when_hash_is_unchanged() -> Result<(), CommonError> {
        let mut fi = fi("demo", None);
        fi.spec.enabled = Some(true);
        let hash = spec_hash(&fi)?;
        fi.status = Some(FrontendIntegrationStatus {
            observed_spec_hash: Some(hash.clone()),
            phase: FrontendIntegrationPhase::Failed,
            ..Default::default()
        });

        assert!(!needs_new_build(&fi, &hash, None));
        Ok(())
    }

    #[test]
    fn builds_when_matching_bundle_is_missing_after_reenable() -> Result<(), CommonError> {
        let mut fi = fi("demo", None);
        fi.spec.enabled = Some(true);
        let hash = spec_hash(&fi)?;
        fi.status = Some(FrontendIntegrationStatus {
            observed_spec_hash: Some(hash.clone()),
            phase: FrontendIntegrationPhase::Pending,
            message: Some("Disabled".to_string()),
            ..Default::default()
        });

        assert!(needs_new_build(&fi, &hash, None));
        Ok(())
    }

    #[test]
    fn hash_label_value_is_dns_safe() {
        assert_eq!(hash_label_value("sha256:abcd"), "abcd");
        assert_eq!(hash_label_value("abcd"), "abcd");
    }

    fn job_with_status(active: Option<i32>, succeeded: Option<i32>, failed: Option<i32>) -> Job {
        Job {
            status: Some(JobStatus {
                active,
                succeeded,
                failed,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn does_not_reuse_failed_job_when_retrying_failed_phase() {
        let fi = fi(
            "demo",
            Some(FrontendIntegrationStatus {
                phase: FrontendIntegrationPhase::Failed,
                ..Default::default()
            }),
        );
        let failed_job = job_with_status(None, None, Some(1));

        assert!(!should_reuse_build_job(
            &fi,
            &failed_job,
            None,
            "sha256:demo"
        ));
    }

    #[test]
    fn reuses_running_job_when_retrying_failed_phase() {
        let fi = fi(
            "demo",
            Some(FrontendIntegrationStatus {
                phase: FrontendIntegrationPhase::Failed,
                ..Default::default()
            }),
        );
        let running_job = job_with_status(Some(1), None, None);

        assert!(should_reuse_build_job(
            &fi,
            &running_job,
            None,
            "sha256:demo"
        ));
    }

    #[test]
    fn does_not_reuse_succeeded_job_when_matching_bundle_is_missing() {
        let fi = fi(
            "demo",
            Some(FrontendIntegrationStatus {
                phase: FrontendIntegrationPhase::Succeeded,
                ..Default::default()
            }),
        );
        let succeeded_job = job_with_status(None, Some(1), None);

        assert!(!should_reuse_build_job(
            &fi,
            &succeeded_job,
            None,
            "sha256:demo",
        ));
    }

    #[test]
    fn bundle_hash_match_uses_build_hash_label() -> Result<(), CommonError> {
        let mut fi = fi("demo", None);
        fi.spec.enabled = Some(true);
        let hash = spec_hash(&fi)?;
        let bundle = bundle_for_hash("fi-demo", &hash);

        assert!(bundle_matches_spec_hash(&bundle, &hash));
        Ok(())
    }

    #[test]
    fn disabled_status_clears_last_build_and_uses_live_bundle_ref() {
        let fi = fi(
            "demo",
            Some(FrontendIntegrationStatus {
                observed_spec_hash: Some("sha256:demo".to_string()),
                observed_manifest_hash: Some("sha256:manifest".to_string()),
                last_build: Some(LastBuildStatus {
                    job_ref: Some(ResourceRef {
                        name: "old-job".to_string(),
                        namespace: Some("default".to_string()),
                        uid: Some("job-uid".to_string()),
                    }),
                    started_at: Some(Utc::now()),
                }),
                bundle_ref: Some(ResourceRef {
                    name: "stale-bundle".to_string(),
                    namespace: None,
                    uid: Some("stale-uid".to_string()),
                }),
                ..Default::default()
            }),
        );
        let bundle = bundle_for_hash("fi-demo", "sha256:demo");
        let status = disabled_status(&fi, Some(&bundle));

        assert!(status.last_build.is_none());
        assert_eq!(
            status.bundle_ref.map(|bundle_ref| bundle_ref.name),
            Some("fi-demo".to_string())
        );
        assert_eq!(status.message.as_deref(), Some("Disabled"));
    }

    #[test]
    fn status_patch_sets_null_for_cleared_optional_refs() -> Result<(), Error> {
        let status = FrontendIntegrationStatus {
            phase: FrontendIntegrationPhase::Pending,
            last_build: None,
            bundle_ref: None,
            ..Default::default()
        };
        let patch = frontend_integration_status_patch(&status, "default", "demo")?;

        assert_eq!(patch["status"]["last_build"], serde_json::Value::Null);
        assert_eq!(patch["status"]["bundle_ref"], serde_json::Value::Null);
        Ok(())
    }
}
