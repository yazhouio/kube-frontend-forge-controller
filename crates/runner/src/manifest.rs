#[path = "manifest/v1.rs"]
mod v1;

use frontend_forge_api::{FrontendIntegration, ManifestRenderError};
use kube::ResourceExt;
use serde_json::Value;

// Runner-local manifest rendering entrypoint. The Job reads FI and derives manifest at runtime.
// Different engine versions can map to different renderers over time.
pub fn render_extension_manifest(fi: &FrontendIntegration) -> Result<Value, ManifestRenderError> {
    let requested = fi.spec.engine_version().unwrap_or("v1").trim();
    let normalized = if requested.is_empty() {
        "v1"
    } else {
        requested
    }
    .to_ascii_lowercase();

    match normalized.as_str() {
        "v1" | "v1alpha1" | "1" | "1.0" => v1::render_v1_manifest(fi),
        _ => Err(ManifestRenderError::UnsupportedEngineVersion {
            fi_name: fi.name_any(),
            engine_version: requested.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn defaults_to_v1_renderer() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  menus:
    - displayName: Demo
      key: demo
      placement: global
      type: page
  pages:
    - key: demo
      type: iframe
      iframe:
        src: http://example.test
"#,
        )
        .unwrap();

        let manifest = render_extension_manifest(&fi).unwrap();
        assert_eq!(manifest["version"], "1.0");
    }

    #[test]
    fn rejects_unknown_engine_version() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  builder:
    engineVersion: v99
  menus:
    - displayName: Demo
      key: demo
      placement: global
      type: page
  pages:
    - key: demo
      type: iframe
      iframe:
        src: http://example.test
"#,
        )
        .unwrap();

        assert!(matches!(
            render_extension_manifest(&fi),
            Err(ManifestRenderError::UnsupportedEngineVersion { .. })
        ));
    }
}
