mod manifest;

use frontend_forge_api::{
    FrontendIntegration, JSBundle, JsBundleNamespacedKeyRef, JsBundleRawFromSpec, JsBundleSpec,
    JsBundleStatus, ManifestRenderError,
};
use frontend_forge_common::{
    ANNO_BUILD_JOB, ANNO_MANIFEST_CONTENT, ANNO_MANIFEST_HASH, CommonError, LABEL_ENABLED,
    LABEL_FI_NAME, LABEL_MANAGED_BY, LABEL_MANIFEST_HASH, LABEL_SPEC_HASH, MANAGED_BY_VALUE,
    bounded_name, hash_label_value, manifest_content_and_hash, serializable_hash,
};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client, Resource};
use serde::Deserialize;
use serde_json::json;
use snafu::{ResultExt, Snafu};
use std::collections::BTreeMap;
use std::env;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("missing env {key}: {source}"))]
    MissingEnv {
        key: &'static str,
        source: std::env::VarError,
    },
    #[snafu(display("invalid env {key}: {source}"))]
    InvalidEnv {
        key: &'static str,
        source: std::num::ParseIntError,
    },
    #[snafu(display("failed to initialize Kubernetes client in runner: {source}"))]
    KubeClientInit { source: kube::Error },
    #[snafu(display("failed to read FrontendIntegration {namespace}/{name}: {source}"))]
    GetFrontendIntegration {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to upsert bundle ConfigMap {namespace}/{name}: {source}"))]
    UpsertBundleConfigMap {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to upsert JSBundle {namespace}/{name}: {source}"))]
    UpsertJsBundle {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to patch JSBundle status {namespace}/{name}: {source}"))]
    PatchJsBundleStatus {
        namespace: String,
        name: String,
        source: kube::Error,
    },
    #[snafu(display("failed to render ExtensionManifest from FrontendIntegration: {source}"))]
    RenderManifest { source: ManifestRenderError },
    #[snafu(display("failed to canonicalize/hash runner manifest: {source}"))]
    ManifestHash { source: CommonError },
    #[snafu(display("failed to canonicalize/hash FrontendIntegration spec: {source}"))]
    SpecHash { source: CommonError },
    #[snafu(display(
        "failed to initialize build-service HTTP client (timeout={timeout_seconds}s): {source}"
    ))]
    BuildServiceClientInit {
        timeout_seconds: u64,
        source: reqwest::Error,
    },
    #[snafu(display("build-service request failed during {operation} {url}: {source}"))]
    BuildServiceRequest {
        operation: &'static str,
        url: String,
        source: reqwest::Error,
    },
    #[snafu(display("build-service returned non-success during {operation} {url}: {source}"))]
    BuildServiceResponseStatus {
        operation: &'static str,
        url: String,
        source: reqwest::Error,
    },
    #[snafu(display("failed to decode build-service response during {operation} {url}: {source}"))]
    BuildServiceDecode {
        operation: &'static str,
        url: String,
        source: reqwest::Error,
    },
    #[snafu(display("build-service returned failure: {message}"))]
    BuildFailed { message: String },
    #[snafu(display("no suitable JS bundle artifact found (wanted key '{desired_key}')"))]
    MissingBundleArtifact { desired_key: String },
    #[snafu(display("fi status.observed_spec_hash not available within grace period"))]
    StaleCheckTimeout,
}

#[derive(Clone, Debug)]
struct RunnerConfig {
    fi_name: String,
    spec_hash: String,
    jsbundle_name: String,
    jsbundle_configmap_namespace: String,
    jsbundle_config_key: String,
    build_service_base_url: String,
    build_service_timeout_seconds: u64,
    stale_check_grace_seconds: u64,
}

impl RunnerConfig {
    fn from_env() -> Result<Self, Error> {
        Ok(Self {
            fi_name: required_env("FI_NAME")?,
            spec_hash: required_env_alias("SPEC_HASH", "MANIFEST_HASH")?,
            jsbundle_name: required_env("JSBUNDLE_NAME")?,
            jsbundle_configmap_namespace: env::var("JSBUNDLE_CONFIGMAP_NAMESPACE")
                .unwrap_or_else(|_| "extension-frontend-forge".to_string()),
            jsbundle_config_key: env::var("JSBUNDLE_CONFIG_KEY")
                .unwrap_or_else(|_| "index.js".to_string()),
            build_service_base_url: required_env("BUILD_SERVICE_BASE_URL")?,
            build_service_timeout_seconds: parse_env_u64("BUILD_SERVICE_TIMEOUT_SECONDS", 600)?,
            stale_check_grace_seconds: parse_env_u64("STALE_CHECK_GRACE_SECONDS", 30)?,
        })
    }
}

fn required_env(key: &'static str) -> Result<String, Error> {
    env::var(key).context(MissingEnvSnafu { key })
}

fn required_env_alias(primary: &'static str, legacy: &'static str) -> Result<String, Error> {
    match env::var(primary) {
        Ok(v) => Ok(v),
        Err(_) => required_env(legacy),
    }
}

fn parse_env_u64(key: &'static str, default: u64) -> Result<u64, Error> {
    match env::var(key) {
        Ok(v) => v.parse::<u64>().context(InvalidEnvSnafu { key }),
        Err(_) => Ok(default),
    }
}

fn enabled_label_value(enabled: bool) -> &'static str {
    if enabled { "true" } else { "false" }
}

fn build_spec_hash(fi: &FrontendIntegration) -> Result<String, CommonError> {
    serializable_hash(&fi.spec.without_enabled())
}

#[derive(Clone)]
struct BuildServiceClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ProjectBuildResponse {
    ok: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    files: Vec<RemoteFile>,
}

#[derive(Debug, Deserialize)]
struct RemoteFile {
    path: String,
    content: String,
}

impl BuildServiceClient {
    fn new(cfg: &RunnerConfig) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.build_service_timeout_seconds))
            .build()
            .context(BuildServiceClientInitSnafu {
                timeout_seconds: cfg.build_service_timeout_seconds,
            })?;
        Ok(Self {
            base_url: cfg.build_service_base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    async fn build_project(&self, manifest: &str) -> Result<Vec<RemoteFile>, Error> {
        let url = format!("{}/api/project/build", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(manifest.to_string())
            .send()
            .await
            .context(BuildServiceRequestSnafu {
                operation: "project_build",
                url: url.clone(),
            })?;
        let resp = resp
            .error_for_status()
            .context(BuildServiceResponseStatusSnafu {
                operation: "project_build",
                url: url.clone(),
            })?;
        let payload: ProjectBuildResponse = resp.json().await.context(BuildServiceDecodeSnafu {
            operation: "project_build",
            url,
        })?;
        if !payload.ok {
            return Err(Error::BuildFailed {
                message: payload
                    .message
                    .unwrap_or_else(|| "build-service returned ok=false".to_string()),
            });
        }
        Ok(payload.files)
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,frontend_forge_runner=debug".into()),
        )
        .init();

    match run().await {
        Ok(()) => Ok(()),
        Err(err) => {
            error!(error = %err, "runner failed");
            Err(err)
        }
    }
}

async fn run() -> Result<(), Error> {
    let cfg = RunnerConfig::from_env()?;
    let kube = Client::try_default().await.context(KubeClientInitSnafu)?;
    let fi_api = Api::<FrontendIntegration>::all(kube.clone());
    let fi_for_build =
        fi_api
            .get(&cfg.fi_name)
            .await
            .with_context(|_| GetFrontendIntegrationSnafu {
                namespace: "<cluster>".to_string(),
                name: cfg.fi_name.clone(),
            })?;
    let build_spec_hash = build_spec_hash(&fi_for_build).context(SpecHashSnafu)?;
    if cfg.spec_hash != build_spec_hash {
        warn!(
            fi = %cfg.fi_name,
            expected_spec_hash = %cfg.spec_hash,
            actual_build_hash = %build_spec_hash,
            "runner observed newer/different FI spec before build; skipping stale job"
        );
        return Ok(());
    }
    let manifest_value =
        manifest::render_extension_manifest(&fi_for_build).context(RenderManifestSnafu)?;
    let (manifest, manifest_hash) =
        manifest_content_and_hash(&manifest_value).context(ManifestHashSnafu)?;

    let build_client = BuildServiceClient::new(&cfg)?;

    info!(
        fi = %cfg.fi_name,
        spec_hash = %cfg.spec_hash,
        manifest_hash = %manifest_hash,
        "starting build runner"
    );
    let files = build_client.build_project(&manifest).await?;
    info!(files = files.len(), "build artifacts fetched");
    let fi = stale_check(&fi_api, &cfg).await?;
    let Some(fi) = fi else {
        warn!("build became stale; exiting without writing JSBundle");
        return Ok(());
    };

    let (bundle_key, bundle_content) = select_bundle_artifact(&cfg, files)?;
    let configmap_name = bundle_configmap_name(&cfg.jsbundle_name);
    let configmap_api =
        Api::<ConfigMap>::namespaced(kube.clone(), &cfg.jsbundle_configmap_namespace);
    upsert_bundle_configmap(
        &configmap_api,
        &cfg,
        &fi,
        &configmap_name,
        &bundle_key,
        &bundle_content,
        &manifest_hash,
    )
    .await?;

    let bundle_api = Api::<JSBundle>::all(kube);
    upsert_jsbundle(
        &bundle_api,
        &cfg,
        &fi,
        &configmap_name,
        &bundle_key,
        &manifest,
        &manifest_hash,
    )
    .await?;
    info!(bundle = %cfg.jsbundle_name, "jsbundle upserted");
    Ok(())
}

async fn stale_check(
    fi_api: &Api<FrontendIntegration>,
    cfg: &RunnerConfig,
) -> Result<Option<FrontendIntegration>, Error> {
    let deadline = Instant::now() + Duration::from_secs(cfg.stale_check_grace_seconds);

    loop {
        let fi = fi_api
            .get(&cfg.fi_name)
            .await
            .with_context(|_| GetFrontendIntegrationSnafu {
                namespace: "<cluster>".to_string(),
                name: cfg.fi_name.clone(),
            })?;
        let observed = fi
            .status
            .as_ref()
            .and_then(|s| s.observed_spec_hash.as_deref())
            .or_else(|| {
                fi.status
                    .as_ref()
                    .and_then(|s| s.observed_manifest_hash.as_deref())
            });

        match observed {
            Some(hash) if hash == cfg.spec_hash => return Ok(Some(fi)),
            Some(_) => return Ok(None),
            None if Instant::now() < deadline => {
                sleep(Duration::from_secs(2)).await;
            }
            None => return Err(Error::StaleCheckTimeout),
        }
    }
}

async fn upsert_bundle_configmap(
    configmap_api: &Api<ConfigMap>,
    cfg: &RunnerConfig,
    fi: &FrontendIntegration,
    configmap_name: &str,
    bundle_key: &str,
    bundle_content: &str,
    manifest_hash: &str,
) -> Result<(), Error> {
    let mut labels = BTreeMap::new();
    labels.insert(LABEL_MANAGED_BY.to_string(), MANAGED_BY_VALUE.to_string());
    labels.insert(LABEL_FI_NAME.to_string(), cfg.fi_name.clone());
    labels.insert(
        LABEL_SPEC_HASH.to_string(),
        hash_label_value(&cfg.spec_hash),
    );
    labels.insert(
        LABEL_MANIFEST_HASH.to_string(),
        hash_label_value(manifest_hash),
    );

    let mut annotations = BTreeMap::new();
    annotations.insert(ANNO_BUILD_JOB.to_string(), job_name_from_env());
    annotations.insert(ANNO_MANIFEST_HASH.to_string(), manifest_hash.to_string());

    let cm = ConfigMap {
        metadata: kube::core::ObjectMeta {
            name: Some(configmap_name.to_string()),
            namespace: Some(cfg.jsbundle_configmap_namespace.clone()),
            owner_references: owner_refs_for(fi),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            bundle_key.to_string(),
            bundle_content.to_string(),
        )])),
        ..Default::default()
    };

    configmap_api
        .patch(
            configmap_name,
            &PatchParams::apply("frontend-forge-builder-runner").force(),
            &Patch::Apply(&cm),
        )
        .await
        .with_context(|_| UpsertBundleConfigMapSnafu {
            namespace: cfg.jsbundle_configmap_namespace.clone(),
            name: configmap_name.to_string(),
        })?;

    Ok(())
}

async fn upsert_jsbundle(
    bundle_api: &Api<JSBundle>,
    cfg: &RunnerConfig,
    fi: &FrontendIntegration,
    configmap_name: &str,
    bundle_key: &str,
    manifest_content: &str,
    manifest_hash: &str,
) -> Result<(), Error> {
    let mut labels = BTreeMap::new();
    labels.insert(LABEL_MANAGED_BY.to_string(), MANAGED_BY_VALUE.to_string());
    labels.insert(LABEL_FI_NAME.to_string(), cfg.fi_name.clone());
    labels.insert(
        LABEL_ENABLED.to_string(),
        enabled_label_value(fi.spec.enabled()).to_string(),
    );
    labels.insert(
        LABEL_SPEC_HASH.to_string(),
        hash_label_value(&cfg.spec_hash),
    );
    labels.insert(
        LABEL_MANIFEST_HASH.to_string(),
        hash_label_value(manifest_hash),
    );

    let annotations = manifest_annotations(&job_name_from_env(), manifest_content, manifest_hash);

    let bundle = JSBundle {
        metadata: kube::core::ObjectMeta {
            name: Some(cfg.jsbundle_name.clone()),
            owner_references: owner_refs_for(fi),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: JsBundleSpec {
            raw: None,
            raw_from: Some(JsBundleRawFromSpec {
                config_map_key_ref: Some(JsBundleNamespacedKeyRef {
                    key: bundle_key.to_string(),
                    name: configmap_name.to_string(),
                    namespace: cfg.jsbundle_configmap_namespace.clone(),
                    optional: None,
                }),
                secret_key_ref: None,
                url: None,
            }),
        },
        status: None,
    };

    bundle_api
        .patch(
            &cfg.jsbundle_name,
            &PatchParams::apply("frontend-forge-builder-runner").force(),
            &Patch::Apply(&bundle),
        )
        .await
        .with_context(|_| UpsertJsBundleSnafu {
            namespace: "<cluster>".to_string(),
            name: cfg.jsbundle_name.clone(),
        })?;

    let desired_status = JsBundleStatus {
        state: Some("Available".to_string()),
        link: Some(bundle_link(&cfg.jsbundle_name, bundle_key)),
        conditions: vec![],
    };
    patch_jsbundle_status(bundle_api, cfg, &desired_status).await?;

    Ok(())
}

fn manifest_annotations(
    job_name: &str,
    manifest_content: &str,
    manifest_hash: &str,
) -> BTreeMap<String, String> {
    let mut annotations = BTreeMap::new();
    annotations.insert(ANNO_BUILD_JOB.to_string(), job_name.to_string());
    annotations.insert(ANNO_MANIFEST_HASH.to_string(), manifest_hash.to_string());
    annotations.insert(
        ANNO_MANIFEST_CONTENT.to_string(),
        manifest_content.to_string(),
    );
    annotations
}

fn owner_refs_for<T>(obj: &T) -> Option<Vec<OwnerReference>>
where
    T: Resource<DynamicType = ()>,
{
    obj.controller_owner_ref(&()).map(|owner| vec![owner])
}

fn bundle_configmap_name(jsbundle_name: &str) -> String {
    bounded_name(&format!("{}-config", jsbundle_name), 63)
}

fn bundle_link(jsbundle_name: &str, bundle_key: &str) -> String {
    format!(
        "/dist/{}/{}",
        jsbundle_name,
        bundle_key.trim_start_matches('/')
    )
}

async fn patch_jsbundle_status(
    bundle_api: &Api<JSBundle>,
    cfg: &RunnerConfig,
    status: &JsBundleStatus,
) -> Result<(), Error> {
    let status_patch = json!({ "status": status });
    match bundle_api
        .patch_status(
            &cfg.jsbundle_name,
            &PatchParams::default(),
            &Patch::Merge(&status_patch),
        )
        .await
    {
        Ok(_) => Ok(()),
        // Some clusters expose JSBundle without status subresource.
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            bundle_api
                .patch(
                    &cfg.jsbundle_name,
                    &PatchParams::default(),
                    &Patch::Merge(&status_patch),
                )
                .await
                .with_context(|_| PatchJsBundleStatusSnafu {
                    namespace: "<cluster>".to_string(),
                    name: cfg.jsbundle_name.clone(),
                })?;
            Ok(())
        }
        Err(err) => Err(Error::PatchJsBundleStatus {
            namespace: "<cluster>".to_string(),
            name: cfg.jsbundle_name.clone(),
            source: err,
        }),
    }
}

fn select_bundle_artifact(
    cfg: &RunnerConfig,
    remote_files: Vec<RemoteFile>,
) -> Result<(String, String), Error> {
    let desired_key = cfg.jsbundle_config_key.clone();
    let selected_idx = remote_files
        .iter()
        .position(|f| f.path == desired_key)
        .or_else(|| {
            if remote_files.len() == 1 {
                Some(0)
            } else {
                remote_files.iter().position(|f| f.path.ends_with(".js"))
            }
        })
        .ok_or_else(|| Error::MissingBundleArtifact {
            desired_key: desired_key.clone(),
        })?;

    let file =
        remote_files
            .into_iter()
            .nth(selected_idx)
            .ok_or_else(|| Error::MissingBundleArtifact {
                desired_key: desired_key.clone(),
            })?;
    let content = decode_remote_file_to_utf8(&file)?;
    let key = if file.path.contains('/') {
        desired_key
    } else {
        file.path
    };
    Ok((key, content))
}

fn decode_remote_file_to_utf8(remote: &RemoteFile) -> Result<String, Error> {
    Ok(remote.content.clone())
}

fn job_name_from_env() -> String {
    env::var("HOSTNAME").unwrap_or_else(|_| "unknown-job".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontend_forge_api::{
        FrontendIntegrationSpec, IframePageSpec, MenuNodeType, MenuPlacement, PageSpec, PageType,
        PrimaryMenuSpec,
    };
    use kube::core::ObjectMeta;

    fn test_fi(name: &str) -> FrontendIntegration {
        FrontendIntegration {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: FrontendIntegrationSpec {
                display_name: None,
                enabled: Some(true),
                menus: vec![PrimaryMenuSpec {
                    display_name: name.to_string(),
                    key: name.to_string(),
                    placement: MenuPlacement::Global,
                    type_: MenuNodeType::Page,
                    children: vec![],
                }],
                pages: vec![PageSpec {
                    key: name.to_string(),
                    type_: PageType::Iframe,
                    crd_table: None,
                    iframe: Some(IframePageSpec {
                        src: "http://example.test".to_string(),
                    }),
                }],
                builder: None,
            },
            status: None,
        }
    }

    #[test]
    fn decodes_plain_file_passthrough() {
        let file = RemoteFile {
            path: "index.js".to_string(),
            content: "console.log('ok')".to_string(),
        };

        let decoded = decode_remote_file_to_utf8(&file).unwrap();
        assert_eq!(decoded, "console.log('ok')");
    }

    #[test]
    fn selects_js_fallback_artifact() {
        let cfg = RunnerConfig {
            fi_name: "demo".to_string(),
            spec_hash: "sha256:abc".to_string(),
            jsbundle_name: "fi-demo".to_string(),
            jsbundle_configmap_namespace: "extension-frontend-forge-config".to_string(),
            jsbundle_config_key: "index.js".to_string(),
            build_service_base_url: "http://builder".to_string(),
            build_service_timeout_seconds: 30,
            stale_check_grace_seconds: 30,
        };

        let (key, content) = select_bundle_artifact(
            &cfg,
            vec![
                RemoteFile {
                    path: "style.css".to_string(),
                    content: "body{}".to_string(),
                },
                RemoteFile {
                    path: "bundle/main.js".to_string(),
                    content: "console.log('js')".to_string(),
                },
            ],
        )
        .unwrap();

        assert_eq!(key, "index.js");
        assert_eq!(content, "console.log('js')");
    }

    #[test]
    fn builds_jsbundle_link() {
        assert_eq!(bundle_link("fi-demo", "index.js"), "/dist/fi-demo/index.js");
    }

    #[test]
    fn manifest_annotations_include_hash_and_content() {
        let annotations = manifest_annotations("job-1", "{\"kind\":\"Extension\"}", "sha256:abc");

        assert_eq!(
            annotations.get(ANNO_BUILD_JOB).map(String::as_str),
            Some("job-1")
        );
        assert_eq!(
            annotations.get(ANNO_MANIFEST_HASH).map(String::as_str),
            Some("sha256:abc")
        );
        assert_eq!(
            annotations.get(ANNO_MANIFEST_CONTENT).map(String::as_str),
            Some("{\"kind\":\"Extension\"}")
        );
    }

    #[test]
    fn build_spec_hash_ignores_enabled() -> Result<(), CommonError> {
        let fi_enabled = test_fi("demo");
        let mut fi_disabled = fi_enabled.clone();
        fi_disabled.spec.enabled = Some(false);

        let enabled_hash = build_spec_hash(&fi_enabled)?;
        let disabled_hash = build_spec_hash(&fi_disabled)?;
        assert_eq!(enabled_hash, disabled_hash);
        Ok(())
    }

    #[test]
    fn enabled_label_is_boolean_string() {
        assert_eq!(enabled_label_value(true), "true");
        assert_eq!(enabled_label_value(false), "false");
    }

    #[test]
    fn owner_refs_for_fi_sets_controller_owner_reference() -> Result<(), &'static str> {
        let mut fi = test_fi("demo");
        fi.metadata.uid = Some("fi-uid-123".to_string());

        let owner_refs = owner_refs_for(&fi).ok_or("owner refs missing")?;
        assert_eq!(owner_refs.len(), 1);

        let owner = &owner_refs[0];
        assert_eq!(owner.api_version, "frontend-forge.kubesphere.io/v1alpha1");
        assert_eq!(owner.kind, "FrontendIntegration");
        assert_eq!(owner.name, "demo");
        assert_eq!(owner.uid, "fi-uid-123");
        assert_eq!(owner.controller, Some(true));
        Ok(())
    }
}
