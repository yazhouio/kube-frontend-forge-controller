use frontend_forge_api::{
    ColumnRenderType, ColumnSpec, CrdScope, CrdTablePageSpec, FrontendIntegration,
    FrontendIntegrationSpec, ManifestRenderError, MenuNodeType, MenuPlacement, PageSpec, PageType,
};
use kube::ResourceExt;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

pub(super) fn render_v1_manifest(fi: &FrontendIntegration) -> Result<Value, ManifestRenderError> {
    let fi_name = fi.name_any();
    let display_name = fi
        .spec
        .display_name
        .clone()
        .unwrap_or_else(|| fi_name.clone());
    let description = fi
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("kubesphere.io/description").cloned());
    let resolved_menus = resolve_spec(&fi.spec, &fi_name)?;

    let mut routes = Vec::new();
    let mut menus = Vec::new();
    let mut pages = Vec::new();

    for menu in resolved_menus {
        match menu {
            ResolvedTopMenu::Page(page) => {
                menus.push(render_leaf_menu(&page));
                routes.push(render_route(&fi_name, &page));
                pages.push(render_page(&fi_name, &page)?);
            }
            ResolvedTopMenu::Organization { menu, children } => {
                menus.push(render_organization_menu(&menu));
                for child in children {
                    menus.push(render_leaf_menu(&child));
                    routes.push(render_route(&fi_name, &child));
                    pages.push(render_page(&fi_name, &child)?);
                }
            }
        }
    }

    let mut manifest = Map::new();
    manifest.insert("version".to_string(), json!("1.0"));
    manifest.insert("name".to_string(), json!(fi_name));
    manifest.insert("displayName".to_string(), json!(display_name));
    if let Some(description) = description {
        manifest.insert("description".to_string(), json!(description));
    }
    manifest.insert("routes".to_string(), Value::Array(routes));
    manifest.insert("menus".to_string(), Value::Array(menus));
    manifest.insert("locales".to_string(), json!([]));
    manifest.insert("pages".to_string(), Value::Array(pages));
    manifest.insert(
        "build".to_string(),
        json!({
            "target": "kubesphere-extension",
            "moduleName": fi.name_any(),
            "systemjs": true,
        }),
    );

    Ok(Value::Object(manifest))
}

#[derive(Clone, Debug)]
enum ResolvedTopMenu {
    Page(ResolvedPageBinding),
    Organization {
        menu: ResolvedOrganizationMenu,
        children: Vec<ResolvedPageBinding>,
    },
}

#[derive(Clone, Debug)]
struct ResolvedOrganizationMenu {
    name: String,
    title: String,
    placement: MenuPlacement,
}

#[derive(Clone, Debug)]
struct ResolvedPageBinding {
    title: String,
    placement: MenuPlacement,
    route_suffix: String,
    menu_name: String,
    parent: String,
    page: PageSpec,
}

fn resolve_spec(
    spec: &FrontendIntegrationSpec,
    fi_name: &str,
) -> Result<Vec<ResolvedTopMenu>, ManifestRenderError> {
    let pages_by_key = resolve_pages(spec, fi_name)?;
    let mut top_level_keys = HashSet::new();
    let mut bound_page_keys = HashSet::new();
    let mut resolved = Vec::new();

    for menu in &spec.menus {
        validate_key(fi_name, &menu.key, true)?;
        if !top_level_keys.insert(menu.key.clone()) {
            return Err(ManifestRenderError::DuplicateTopLevelMenuKey {
                fi_name: fi_name.to_string(),
                key: menu.key.clone(),
            });
        }

        match menu.type_ {
            MenuNodeType::Page => {
                let top_menu_name = menu_name_for_suffix(fi_name, &menu.key);
                if !menu.children.is_empty() {
                    return Err(ManifestRenderError::InvalidMenuShape {
                        fi_name: fi_name.to_string(),
                        key: menu.key.clone(),
                        message: "page menus cannot define children".to_string(),
                    });
                }

                let page = bind_page(fi_name, &menu.key, &pages_by_key, &mut bound_page_keys)?;
                resolved.push(ResolvedTopMenu::Page(ResolvedPageBinding {
                    title: menu.display_name.clone(),
                    placement: menu.placement,
                    route_suffix: route_suffix_for_menu(&menu.key),
                    menu_name: top_menu_name,
                    parent: menu.placement.as_str().to_string(),
                    page,
                }));
            }
            MenuNodeType::Organization => {
                let top_menu_name = menu_name_for_suffix(fi_name, &menu.key);
                if menu.children.is_empty() {
                    return Err(ManifestRenderError::InvalidMenuShape {
                        fi_name: fi_name.to_string(),
                        key: menu.key.clone(),
                        message: "organization menus must define at least one child".to_string(),
                    });
                }
                if pages_by_key.contains_key(&menu.key) {
                    return Err(ManifestRenderError::InvalidMenuShape {
                        fi_name: fi_name.to_string(),
                        key: menu.key.clone(),
                        message: "organization menus cannot bind to page configs".to_string(),
                    });
                }

                let mut children = Vec::new();
                for child in &menu.children {
                    validate_key(fi_name, &child.key, true)?;
                    let page = bind_page(fi_name, &child.key, &pages_by_key, &mut bound_page_keys)?;
                    let route_suffix = route_suffix_for_child(&menu.key, &child.key);
                    children.push(ResolvedPageBinding {
                        title: child.display_name.clone(),
                        placement: menu.placement,
                        route_suffix: route_suffix.clone(),
                        menu_name: menu_name_for_suffix(fi_name, &route_suffix),
                        parent: nested_menu_parent(menu.placement, &top_menu_name),
                        page,
                    });
                }

                resolved.push(ResolvedTopMenu::Organization {
                    menu: ResolvedOrganizationMenu {
                        name: top_menu_name,
                        title: menu.display_name.clone(),
                        placement: menu.placement,
                    },
                    children,
                });
            }
        }
    }

    for page in &spec.pages {
        if !bound_page_keys.contains(&page.key) {
            return Err(ManifestRenderError::OrphanPageConfig {
                fi_name: fi_name.to_string(),
                key: page.key.clone(),
            });
        }
    }

    Ok(resolved)
}

fn resolve_pages(
    spec: &FrontendIntegrationSpec,
    fi_name: &str,
) -> Result<HashMap<String, PageSpec>, ManifestRenderError> {
    let mut pages = HashMap::new();

    for page in &spec.pages {
        validate_key(fi_name, &page.key, false)?;
        validate_page_shape(fi_name, page)?;
        if pages.insert(page.key.clone(), page.clone()).is_some() {
            return Err(ManifestRenderError::DuplicatePageKey {
                fi_name: fi_name.to_string(),
                key: page.key.clone(),
            });
        }
    }

    Ok(pages)
}

fn bind_page(
    fi_name: &str,
    key: &str,
    pages_by_key: &HashMap<String, PageSpec>,
    bound_page_keys: &mut HashSet<String>,
) -> Result<PageSpec, ManifestRenderError> {
    if !bound_page_keys.insert(key.to_string()) {
        return Err(ManifestRenderError::DuplicatePageKey {
            fi_name: fi_name.to_string(),
            key: key.to_string(),
        });
    }

    pages_by_key
        .get(key)
        .cloned()
        .ok_or_else(|| ManifestRenderError::MissingPageForMenuKey {
            fi_name: fi_name.to_string(),
            key: key.to_string(),
        })
}

fn validate_key(fi_name: &str, key: &str, is_menu_key: bool) -> Result<(), ManifestRenderError> {
    let is_valid = !key.is_empty()
        && !key.starts_with('-')
        && !key.ends_with('-')
        && key
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');

    if is_valid {
        Ok(())
    } else if is_menu_key {
        Err(ManifestRenderError::InvalidMenuKey {
            fi_name: fi_name.to_string(),
            key: key.to_string(),
        })
    } else {
        Err(ManifestRenderError::InvalidPageShape {
            fi_name: fi_name.to_string(),
            key: key.to_string(),
            message: "page keys must be kebab-case route fragments".to_string(),
        })
    }
}

fn validate_page_shape(fi_name: &str, page: &PageSpec) -> Result<(), ManifestRenderError> {
    match page.type_ {
        PageType::Iframe => {
            if page.iframe.is_none() {
                return Err(ManifestRenderError::InvalidPageShape {
                    fi_name: fi_name.to_string(),
                    key: page.key.clone(),
                    message: "type=iframe requires iframe config".to_string(),
                });
            }
            if page.crd_table.is_some() {
                return Err(ManifestRenderError::InvalidPageShape {
                    fi_name: fi_name.to_string(),
                    key: page.key.clone(),
                    message: "type=iframe cannot define crdTable config".to_string(),
                });
            }
        }
        PageType::CrdTable => {
            let Some(crd_table) = page.crd_table.as_ref() else {
                return Err(ManifestRenderError::InvalidPageShape {
                    fi_name: fi_name.to_string(),
                    key: page.key.clone(),
                    message: "type=crdTable requires crdTable config".to_string(),
                });
            };
            if page.iframe.is_some() {
                return Err(ManifestRenderError::InvalidPageShape {
                    fi_name: fi_name.to_string(),
                    key: page.key.clone(),
                    message: "type=crdTable cannot define iframe config".to_string(),
                });
            }
            if crd_table.columns.is_empty() {
                return Err(ManifestRenderError::MissingCrdColumns {
                    fi_name: fi_name.to_string(),
                    key: page.key.clone(),
                });
            }
        }
    }

    Ok(())
}

fn route_suffix_for_menu(key: &str) -> String {
    key.to_string()
}

fn route_suffix_for_child(parent_key: &str, child_key: &str) -> String {
    format!("{parent_key}/{child_key}")
}

fn menu_name_for_suffix(fi_name: &str, suffix: &str) -> String {
    format!("frontendintegrations/{fi_name}/{suffix}")
}

fn nested_menu_parent(placement: MenuPlacement, menu_name: &str) -> String {
    format!("{}.{}", placement.as_str(), menu_name)
}

fn page_id_for_suffix(fi_name: &str, placement: MenuPlacement, suffix: &str) -> String {
    format!(
        "{}-{}-{}",
        fi_name,
        placement.as_str(),
        suffix.replace('/', "_")
    )
}

fn render_route(fi_name: &str, page: &ResolvedPageBinding) -> Value {
    let page_id = page_id_for_suffix(fi_name, page.placement, &page.route_suffix);
    json!({
        "path": format!(
            "{}{}",
            page.placement.route_prefix(),
            route_tail(fi_name, &page.route_suffix)
        ),
        "pageId": page_id,
    })
}

fn render_leaf_menu(page: &ResolvedPageBinding) -> Value {
    json!({
        "parent": page.parent,
        "name": page.menu_name,
        "title": page.title,
        "icon": "GridDuotone",
        "order": 999,
    })
}

fn render_organization_menu(menu: &ResolvedOrganizationMenu) -> Value {
    json!({
        "parent": menu.placement.as_str(),
        "name": menu.name,
        "title": menu.title,
        "icon": "GridDuotone",
        "order": 999,
    })
}

fn route_tail(fi_name: &str, suffix: &str) -> String {
    format!("/frontendintegrations/{fi_name}/{suffix}")
}

fn render_page(fi_name: &str, page: &ResolvedPageBinding) -> Result<Value, ManifestRenderError> {
    let page_id = page_id_for_suffix(fi_name, page.placement, &page.route_suffix);

    match page.page.type_ {
        PageType::Iframe => {
            let iframe =
                page.page
                    .iframe
                    .as_ref()
                    .ok_or_else(|| ManifestRenderError::InvalidPageShape {
                        fi_name: fi_name.to_string(),
                        key: page.page.key.clone(),
                        message: "type=iframe requires iframe config".to_string(),
                    })?;
            Ok(iframe_page(&page_id, &page.title, &iframe.src))
        }
        PageType::CrdTable => {
            let crd_table = page.page.crd_table.as_ref().ok_or_else(|| {
                ManifestRenderError::InvalidPageShape {
                    fi_name: fi_name.to_string(),
                    key: page.page.key.clone(),
                    message: "type=crdTable requires crdTable config".to_string(),
                }
            })?;
            Ok(crd_page(
                &page_id,
                &page.title,
                page.placement,
                crd_table,
                &crd_table.columns,
            ))
        }
    }
}

fn page_meta(page_id: &str, title: &str) -> Value {
    json!({
      "id": page_id,
      "name": page_id,
      "title": title,
      "path": format!("/{}", page_id),
    })
}

fn iframe_page(page_id: &str, display_name: &str, frame_src: &str) -> Value {
    json!({
      "id": page_id,
      "entryComponent": page_id,
      "componentsTree": {
        "meta": page_meta(page_id, display_name),
        "context": {},
        "root": {
          "id": format!("{}-root", page_id),
          "type": "Iframe",
          "props": {
            "FRAME_URL": frame_src,
          },
          "meta": { "title": "Iframe", "scope": true }
        }
      }
    })
}

fn crd_page(
    page_id: &str,
    display_name: &str,
    placement: MenuPlacement,
    crd: &CrdTablePageSpec,
    columns: &[ColumnSpec],
) -> Value {
    let columns_config = transform_columns(columns);
    let page_state_type = crd_page_state_type(placement);
    let page_state_config = crd_page_state_config(page_id, placement, crd);

    json!({
      "id": page_id,
      "entryComponent": page_id,
      "componentsTree": {
        "meta": page_meta(page_id, display_name),
        "context": {},
        "dataSources": [
          {
            "id": "columns",
            "type": "crd-columns",
            "config": {
              "COLUMNS_CONFIG": columns_config,
              "HOOK_NAME": "useCrdColumns"
            }
          },
          {
            "id": "pageState",
            "type": page_state_type,
            "args": [
              { "type": "binding", "source": "columns", "bind": "columns" }
            ],
            "config": page_state_config
          }
        ],
        "root": {
          "id": format!("{}-root", page_id),
          "type": "CrdTable",
          "props": {
            "TABLE_KEY": page_id,
            "TITLE": display_name,
            "PARAMS": { "type": "binding", "source": "pageState", "bind": "params" },
            "REFETCH": { "type": "binding", "source": "pageState", "bind": "refetch" },
            "TOOLBAR_LEFT": { "type": "binding", "source": "pageState", "bind": "toolbarLeft" },
            "PAGE_CONTEXT": { "type": "binding", "source": "pageState", "bind": "pageContext" },
            "COLUMNS": { "type": "binding", "source": "columns", "bind": "columns" },
            "DATA": { "type": "binding", "source": "pageState", "bind": "data" },
            "IS_LOADING": {
              "type": "binding",
              "source": "pageState",
              "bind": "loading",
              "defaultValue": false
            },
            "UPDATE": { "type": "binding", "source": "pageState", "bind": "update" },
            "DEL": { "type": "binding", "source": "pageState", "bind": "del" },
            "CREATE": { "type": "binding", "source": "pageState", "bind": "create" },
            "CREATE_INITIAL_VALUE": {
              "apiVersion": format!("{}/{}", crd.group, crd.version),
              "kind": crd.names.kind
            }
          },
          "meta": { "title": "CrdTable", "scope": true }
        }
      }
    })
}

fn crd_page_state_type(placement: MenuPlacement) -> &'static str {
    match placement {
        MenuPlacement::Workspace => "workspace-crd-page-state",
        _ => "crd-page-state",
    }
}

fn crd_page_state_config(page_id: &str, placement: MenuPlacement, crd: &CrdTablePageSpec) -> Value {
    let mut config = Map::new();
    config.insert("PAGE_ID".to_string(), json!(page_id));
    config.insert(
        "CRD_CONFIG".to_string(),
        json!({
          "apiVersion": crd.version,
          "kind": crd.names.kind,
          "plural": crd.names.plural,
          "group": crd.group,
          "kapi": true
        }),
    );
    if placement != MenuPlacement::Workspace {
        config.insert("SCOPE".to_string(), json!(crd_page_scope(crd)));
    }
    config.insert("HOOK_NAME".to_string(), json!("useCrdPageState"));
    Value::Object(config)
}

fn crd_page_scope(crd: &CrdTablePageSpec) -> &'static str {
    match crd.scope {
        CrdScope::Namespaced => "namespace",
        CrdScope::Cluster => "cluster",
    }
}

fn transform_columns(columns: &[ColumnSpec]) -> Vec<Value> {
    columns
        .iter()
        .map(|col| {
            let mut payload = payload_object(col.render.payload.as_ref());
            if let Some(format) = &col.render.format {
                payload.insert("format".to_string(), json!(format));
            }
            if let Some(pattern) = &col.render.pattern {
                payload.insert("pattern".to_string(), json!(pattern));
            }
            if let Some(link) = &col.render.link {
                payload.insert("link".to_string(), json!(link));
            }

            let mut out = Map::new();
            out.insert("key".to_string(), json!(col.key));
            out.insert("title".to_string(), json!(col.title));
            out.insert(
                "render".to_string(),
                json!({
                  "type": render_type_str(&col.render.type_),
                  "path": col.render.path,
                  "payload": Value::Object(payload),
                }),
            );
            if let Some(v) = col.enable_sorting {
                out.insert("enableSorting".to_string(), json!(v));
            }
            if let Some(v) = col.enable_hiding {
                out.insert("enableHiding".to_string(), json!(v));
            }
            Value::Object(out)
        })
        .collect()
}

fn payload_object(payload: Option<&Map<String, Value>>) -> Map<String, Value> {
    payload.cloned().unwrap_or_default()
}

fn render_type_str(t: &ColumnRenderType) -> &'static str {
    match t {
        ColumnRenderType::Text => "text",
        ColumnRenderType::Time => "time",
        ColumnRenderType::Link => "link",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_workspace_crd_pages_with_workspace_page_state() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: test
spec:
  menus:
    - displayName: Cluster Tasks
      key: cluster-tasks
      placement: cluster
      type: page
    - displayName: Workspace Tasks
      key: workspace-tasks
      placement: workspace
      type: page
  pages:
    - key: cluster-tasks
      type: crdTable
      crdTable:
        names:
          plural: serviceaccounts
          kind: ServiceAccount
        version: v1alpha1
        group: kubesphere.io
        scope: Namespaced
        columns:
          - key: name
            title: NAME
            enableSorting: true
            render:
              type: text
              path: metadata.name
    - key: workspace-tasks
      type: crdTable
      crdTable:
        names:
          plural: serviceaccounts
          kind: ServiceAccount
        version: v1alpha1
        group: kubesphere.io
        scope: Namespaced
        columns:
          - key: name
            title: NAME
            enableSorting: true
            render:
              type: text
              path: metadata.name
"#,
        )
        .unwrap();

        let manifest = render_v1_manifest(&fi).unwrap();
        let pages = manifest["pages"].as_array().unwrap();

        let cluster_page_state = &pages[0]["componentsTree"]["dataSources"][1];
        assert_eq!(cluster_page_state["type"], "crd-page-state");
        assert_eq!(
            cluster_page_state["config"]["PAGE_ID"],
            "test-cluster-cluster-tasks"
        );
        assert_eq!(cluster_page_state["config"]["SCOPE"], "namespace");

        let workspace_page_state = &pages[1]["componentsTree"]["dataSources"][1];
        assert_eq!(workspace_page_state["type"], "workspace-crd-page-state");
        assert_eq!(
            workspace_page_state["config"]["PAGE_ID"],
            "test-workspace-workspace-tasks"
        );
        assert!(workspace_page_state["config"].get("SCOPE").is_none());
    }

    #[test]
    fn renders_nested_org_menu_bindings() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo-fi
spec:
  displayName: Demo
  menus:
    - displayName: Ops
      key: ops
      placement: workspace
      type: organization
      children:
        - displayName: Inspect Tasks
          key: inspecttasks
        - displayName: Ops Guide
          key: ops-guide
  pages:
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
    - key: ops-guide
      type: iframe
      iframe:
        src: http://example.test/ops-guide
"#,
        )
        .unwrap();

        let manifest = render_v1_manifest(&fi).unwrap();
        let menus = manifest["menus"].as_array().unwrap();
        let routes = manifest["routes"].as_array().unwrap();
        let pages = manifest["pages"].as_array().unwrap();

        assert_eq!(menus.len(), 3);
        assert_eq!(routes.len(), 2);
        assert_eq!(pages.len(), 2);
        assert_eq!(menus[0]["name"], "frontendintegrations/demo-fi/ops");
        assert_eq!(menus[0]["parent"], "workspace");
        assert_eq!(
            menus[1]["parent"],
            "workspace.frontendintegrations/demo-fi/ops"
        );
        assert_eq!(
            menus[1]["name"],
            "frontendintegrations/demo-fi/ops/inspecttasks"
        );
        assert_eq!(
            menus[2]["parent"],
            "workspace.frontendintegrations/demo-fi/ops"
        );
        assert_eq!(
            menus[2]["name"],
            "frontendintegrations/demo-fi/ops/ops-guide"
        );
        assert_eq!(
            routes[0]["path"],
            "/workspaces/:workspace/frontendintegrations/demo-fi/ops/inspecttasks"
        );
        assert_eq!(routes[0]["pageId"], "demo-fi-workspace-ops_inspecttasks");
        assert_eq!(
            routes[1]["path"],
            "/workspaces/:workspace/frontendintegrations/demo-fi/ops/ops-guide"
        );
        assert_eq!(routes[1]["pageId"], "demo-fi-workspace-ops_ops-guide");
        assert_eq!(pages[0]["componentsTree"]["meta"]["title"], "Inspect Tasks");
        assert_eq!(
            pages[0]["componentsTree"]["dataSources"][1]["type"],
            "workspace-crd-page-state"
        );
        assert_eq!(pages[1]["componentsTree"]["meta"]["title"], "Ops Guide");
    }

    #[test]
    fn keeps_distinct_page_ids_for_top_level_and_nested_suffixes() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo-fi
spec:
  menus:
    - displayName: Ops Guide
      key: ops-guide
      placement: workspace
      type: page
    - displayName: Ops
      key: ops
      placement: workspace
      type: organization
      children:
        - displayName: Guide
          key: guide
  pages:
    - key: ops-guide
      type: iframe
      iframe:
        src: http://example.test/top-level
    - key: guide
      type: iframe
      iframe:
        src: http://example.test/nested
"#,
        )
        .unwrap();

        let manifest = render_v1_manifest(&fi).unwrap();
        let routes = manifest["routes"].as_array().unwrap();
        let pages = manifest["pages"].as_array().unwrap();

        assert_eq!(routes[0]["pageId"], "demo-fi-workspace-ops-guide");
        assert_eq!(routes[1]["pageId"], "demo-fi-workspace-ops_guide");
        assert_ne!(routes[0]["pageId"], routes[1]["pageId"]);
        assert_eq!(pages[0]["id"], "demo-fi-workspace-ops-guide");
        assert_eq!(pages[1]["id"], "demo-fi-workspace-ops_guide");
    }

    #[test]
    fn rejects_page_menu_with_children() {
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
      children:
        - displayName: Child
          key: child
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test
"#,
        )
        .unwrap();

        assert!(matches!(
            render_v1_manifest(&fi),
            Err(ManifestRenderError::InvalidMenuShape { .. })
        ));
    }

    #[test]
    fn rejects_org_menu_without_children() {
        let fi: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  menus:
    - displayName: Ops
      key: ops
      placement: cluster
      type: organization
  pages: []
"#,
        )
        .unwrap();

        assert!(matches!(
            render_v1_manifest(&fi),
            Err(ManifestRenderError::InvalidMenuShape { .. })
        ));
    }

    #[test]
    fn rejects_missing_page_for_menu_key() {
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
  pages: []
"#,
        )
        .unwrap();

        assert!(matches!(
            render_v1_manifest(&fi),
            Err(ManifestRenderError::MissingPageForMenuKey { .. })
        ));
    }

    #[test]
    fn rejects_orphan_page_config() {
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
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test
    - key: orphan
      type: iframe
      iframe:
        src: http://example.test/orphan
"#,
        )
        .unwrap();

        assert!(matches!(
            render_v1_manifest(&fi),
            Err(ManifestRenderError::OrphanPageConfig { .. })
        ));
    }

    #[test]
    fn rejects_invalid_page_shapes_and_keys() {
        let invalid_menu_key: FrontendIntegration = serde_yaml::from_str(
            r#"
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo
spec:
  menus:
    - displayName: Overview
      key: invalid_key
      placement: cluster
      type: page
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test
"#,
        )
        .unwrap();
        assert!(matches!(
            render_v1_manifest(&invalid_menu_key),
            Err(ManifestRenderError::InvalidMenuKey { .. })
        ));

        let invalid_page_key: FrontendIntegration = serde_yaml::from_str(
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
  pages:
    - key: invalid_key
      type: iframe
      iframe:
        src: http://example.test
"#,
        )
        .unwrap();
        assert!(matches!(
            render_v1_manifest(&invalid_page_key),
            Err(ManifestRenderError::InvalidPageShape { .. })
        ));

        let missing_iframe: FrontendIntegration = serde_yaml::from_str(
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
  pages:
    - key: overview
      type: iframe
"#,
        )
        .unwrap();
        assert!(matches!(
            render_v1_manifest(&missing_iframe),
            Err(ManifestRenderError::InvalidPageShape { .. })
        ));

        let missing_columns: FrontendIntegration = serde_yaml::from_str(
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
  pages:
    - key: overview
      type: crdTable
      crdTable:
        names:
          plural: serviceaccounts
          kind: ServiceAccount
        version: v1alpha1
        group: kubesphere.io
        scope: Namespaced
        columns: []
"#,
        )
        .unwrap();
        assert!(matches!(
            render_v1_manifest(&missing_columns),
            Err(ManifestRenderError::MissingCrdColumns { .. })
        ));
    }
}
