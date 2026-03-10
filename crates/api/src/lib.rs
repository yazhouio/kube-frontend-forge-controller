use chrono::{DateTime, Utc};
use kube::CustomResource;
use kube::CustomResourceExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use snafu::Snafu;
use std::collections::BTreeMap;

pub const API_GROUP: &str = "frontend-forge.kubesphere.io";
pub const API_VERSION: &str = "v1alpha1";
pub const JSBUNDLE_PLURAL: &str = "jsbundles";
pub const JSBUNDLE_API_GROUP: &str = "extensions.kubesphere.io";
pub const JSBUNDLE_API_VERSION: &str = "v1alpha1";
pub const RESOURCE_SERVED_LABEL_KEY: &str = "kubesphere.io/resource-served";
pub const RESOURCE_SERVED_LABEL_VALUE: &str = "true";

#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[kube(
    group = "frontend-forge.kubesphere.io",
    version = "v1alpha1",
    kind = "FrontendIntegration",
    plural = "frontendintegrations",
    status = "FrontendIntegrationStatus",
    shortname = "fi"
)]
pub struct FrontendIntegrationSpec {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "displayName"
    )]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locales: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub menus: Vec<PrimaryMenuSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder: Option<BuilderSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BuilderSpec {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "engineVersion"
    )]
    pub engine_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PrimaryMenuSpec {
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub key: String,
    pub placement: MenuPlacement,
    #[serde(rename = "type")]
    pub type_: MenuNodeType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SecondaryMenuSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SecondaryMenuSpec {
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MenuNodeType {
    Page,
    Organization,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PageSpec {
    pub key: String,
    #[serde(rename = "type")]
    pub type_: PageType,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "crdTable")]
    pub crd_table: Option<CrdTablePageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iframe: Option<IframePageSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum PageType {
    #[serde(rename = "crdTable")]
    #[schemars(rename = "crdTable")]
    CrdTable,
    #[serde(rename = "iframe")]
    #[schemars(rename = "iframe")]
    Iframe,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IframePageSpec {
    #[serde(alias = "url")]
    pub src: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CrdTablePageSpec {
    pub names: CrdNamesSpec,
    pub group: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "authKey")]
    pub auth_key: Option<String>,
    pub scope: CrdScope,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CrdNamesSpec {
    pub kind: String,
    pub plural: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum CrdScope {
    Namespaced,
    Cluster,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ColumnSpec {
    pub key: String,
    pub title: String,
    pub render: ColumnRenderSpec,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "enableSorting"
    )]
    pub enable_sorting: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "enableHiding"
    )]
    pub enable_hiding: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ColumnRenderSpec {
    #[serde(rename = "type")]
    pub type_: ColumnRenderType,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ColumnRenderType {
    Text,
    Time,
    Link,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MenuPlacement {
    Global,
    Workspace,
    Cluster,
}

#[derive(Debug, Snafu)]
pub enum ManifestRenderError {
    #[snafu(display(
        "FrontendIntegration {} has duplicate top-level menu key '{}'",
        fi_name,
        key
    ))]
    DuplicateTopLevelMenuKey { fi_name: String, key: String },
    #[snafu(display("FrontendIntegration {} has duplicate page key '{}'", fi_name, key))]
    DuplicatePageKey { fi_name: String, key: String },
    #[snafu(display(
        "FrontendIntegration {} is missing page config for menu key '{}'",
        fi_name,
        key
    ))]
    MissingPageForMenuKey { fi_name: String, key: String },
    #[snafu(display(
        "FrontendIntegration {} has page config '{}' without a menu binding",
        fi_name,
        key
    ))]
    OrphanPageConfig { fi_name: String, key: String },
    #[snafu(display(
        "FrontendIntegration {} has invalid menu shape for key '{}': {}",
        fi_name,
        key,
        message
    ))]
    InvalidMenuShape {
        fi_name: String,
        key: String,
        message: String,
    },
    #[snafu(display(
        "FrontendIntegration {} has invalid page shape for key '{}': {}",
        fi_name,
        key,
        message
    ))]
    InvalidPageShape {
        fi_name: String,
        key: String,
        message: String,
    },
    #[snafu(display("FrontendIntegration {} has invalid menu key '{}'", fi_name, key))]
    InvalidMenuKey { fi_name: String, key: String },
    #[snafu(display(
        "FrontendIntegration {} requires columns for CRD page '{}'",
        fi_name,
        key
    ))]
    MissingCrdColumns { fi_name: String, key: String },
    #[snafu(display(
        "FrontendIntegration {} requested unsupported builder.engineVersion '{}'",
        fi_name,
        engine_version
    ))]
    UnsupportedEngineVersion {
        fi_name: String,
        engine_version: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum FrontendIntegrationPhase {
    #[default]
    Pending,
    Building,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub struct ResourceRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub struct LastBuildStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub struct SimpleCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
pub struct FrontendIntegrationStatus {
    #[serde(default)]
    pub phase: FrontendIntegrationPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_spec_hash: Option<String>,
    // Deprecated compatibility field from earlier MVPs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "active_build"
    )]
    pub last_build: Option<LastBuildStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_ref: Option<ResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<SimpleCondition>,
}

#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[kube(
    group = "extensions.kubesphere.io",
    version = "v1alpha1",
    kind = "JSBundle",
    plural = "jsbundles",
    status = "JsBundleStatus"
)]
pub struct JsBundleSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "rawFrom")]
    pub raw_from: Option<JsBundleRawFromSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct JsBundleRawFromSpec {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "configMapKeyRef"
    )]
    pub config_map_key_ref: Option<JsBundleNamespacedKeyRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "secretKeyRef"
    )]
    pub secret_key_ref: Option<JsBundleNamespacedKeyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct JsBundleNamespacedKeyRef {
    pub key: String,
    pub name: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
pub struct JsBundleStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Value>,
}

impl FrontendIntegrationSpec {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    pub fn without_enabled(&self) -> Self {
        let mut spec = self.clone();
        spec.enabled = None;
        spec
    }

    pub fn engine_version(&self) -> Option<&str> {
        self.builder
            .as_ref()
            .and_then(|builder| builder.engine_version.as_deref())
    }
}

pub fn frontend_integration_crd()
-> k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition {
    let mut crd = FrontendIntegration::crd();
    crd.metadata
        .labels
        .get_or_insert_with(BTreeMap::new)
        .insert(
            RESOURCE_SERVED_LABEL_KEY.to_string(),
            RESOURCE_SERVED_LABEL_VALUE.to_string(),
        );
    crd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_page_and_org_menu_spec() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  menus:
    - displayName: Overview
      key: overview
      placement: cluster
      type: page
    - displayName: Ops
      key: ops
      placement: workspace
      type: organization
      children:
        - displayName: Inspect Tasks
          key: inspecttasks
        - displayName: Inspect Rules
          key: inspectrules
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test/overview
    - key: inspecttasks
      type: crdTable
      crdTable:
        names:
          plural: inspecttasks
          kind: InspectTask
        group: kubeeye.kubesphere.io
        version: v1alpha2
        scope: Cluster
        columns:
          - key: name
            title: NAME
            render:
              type: text
              path: metadata.name
    - key: inspectrules
      type: crdTable
      crdTable:
        names:
          plural: inspectrules
          kind: InspectRule
        group: kubeeye.kubesphere.io
        version: v1alpha2
        scope: Cluster
        columns:
          - key: name
            title: NAME
            render:
              type: text
              path: metadata.name
"#,
        )
        .unwrap();

        assert_eq!(fi.spec.menus.len(), 2);
        assert_eq!(fi.spec.pages.len(), 3);
        assert_eq!(fi.spec.pages[1].type_, PageType::CrdTable);
    }

    #[test]
    fn deserializes_optional_display_name_and_locales() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  locales:
    zh:
      xx: Chinese
      yy: Chinese 2
    en:
      xx: English
      yy: English 2
  menus:
    - displayName: Overview
      key: overview
      placement: cluster
      type: page
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test/overview
"#,
        )
        .unwrap();

        assert!(fi.spec.display_name.is_none());
        assert_eq!(
            fi.spec
                .locales
                .get("zh")
                .and_then(|messages| messages.get("xx"))
                .map(String::as_str),
            Some("Chinese")
        );
        assert_eq!(
            fi.spec
                .locales
                .get("en")
                .and_then(|messages| messages.get("yy"))
                .map(String::as_str),
            Some("English 2")
        );
    }

    #[test]
    fn generated_crd_drops_legacy_fields() {
        let crd = frontend_integration_crd();
        let schema = serde_json::to_value(&crd).unwrap();
        let spec_properties = &schema["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]
            ["spec"]["properties"];

        assert!(spec_properties.get("locales").is_some());
        assert!(spec_properties.get("menus").is_some());
        assert!(spec_properties.get("pages").is_some());
        assert!(spec_properties.get("integration").is_none());
        assert!(spec_properties.get("routing").is_none());
        assert!(spec_properties.get("columns").is_none());
        assert!(spec_properties.get("menu").is_none());
    }

    #[test]
    fn generated_crd_sets_resource_served_label() {
        let crd = frontend_integration_crd();

        assert_eq!(
            crd.metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(RESOURCE_SERVED_LABEL_KEY)),
            Some(&RESOURCE_SERVED_LABEL_VALUE.to_string())
        );
    }
}

impl MenuPlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            MenuPlacement::Global => "global",
            MenuPlacement::Workspace => "workspace",
            MenuPlacement::Cluster => "cluster",
        }
    }

    pub fn route_prefix(self) -> &'static str {
        match self {
            MenuPlacement::Cluster => "/clusters/:cluster",
            MenuPlacement::Workspace => "/workspaces/:workspace",
            MenuPlacement::Global => "",
        }
    }
}
