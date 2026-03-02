# Manifest 定义

```typescript
export type RouteMeta = {
  path: string;
  pageId: string;
};

export type MenuMeta = {
  parent: string;
  name: string;
  title: string;
  icon?: string;
  order?: number;
  clusterModule?: string;
};

export type LocaleMeta = {
  lang: string;
  messages: Record<string, string>;
};

export type ManifestPageMeta = {
  id: string;
  entryComponent: string;
  componentsTree: PageConfig;
};

export type ExtensionManifest = {
  version: "1.0";
  name: string;
  displayName?: string;
  description?: string;
  routes: RouteMeta[];
  menus: MenuMeta[];
  locales: LocaleMeta[];
  pages: ManifestPageMeta[];
  build?: {
    target: "kubesphere-extension";
    moduleName?: string;
    namespace?: string;
    cluster?: string;
    systemjs?: boolean;
  };
};

export interface PageConfig {
  meta: PageConfigMeta;
  dataSources?: DataSourceNode[];
  root: ComponentNode;
  context: Record<string, any>;
}

export interface PageConfigMeta {
  id: string;
  name: string;
  title?: string;
  description?: string;
  path?: string;
}

export interface DataSourceNode {
  id: string;
  type: string;
  config: Record<string, any>;
  args?: PropValue[];
  autoLoad?: boolean;
  polling?: {
    enabled: boolean;
    interval?: number;
  };
}

export interface ComponentNode {
  id: string;
  type: string;
  props?: Record<string, PropValue>;
  meta?: {
    scope: boolean;
    title?: string;
  };
  children?: ComponentNode[];
}

export type PropValue =
  | string
  | number
  | boolean
  | object
  | BindingValue
  | ExpressionValue;

export interface BindingValue {
  type: "binding";
  source?: string;
  bind?: string;
  target?: "context" | "dataSource" | "runtime";
  path?: string;
  defaultValue?: any;
}

export interface ExpressionValue {
  type: "expression";
  code: string;
  deps?: ExpressionDeps;
}

export interface ExpressionDeps {
  dataSources?: string[];
  runtime?: true;
  capabilities?: string[];
}
```

## 1. 输入 CR 示例

```yaml
apiVersion: frontend-forge.kubesphere.io/v1alpha1
kind: FrontendIntegration
metadata:
  name: demo-fi
  annotations:
    kubesphere.io/description: Demo multi-page integration
spec:
  displayName: Demo FI
  enabled: true
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
  pages:
    - key: overview
      type: iframe
      iframe:
        src: http://example.test/frontend
    - key: inspecttasks
      type: crdTable
      crdTable:
        names:
          plural: inspecttasks
          kind: InspectTask
        version: v1alpha2
        group: kubeeye.kubesphere.io
        scope: Cluster
        columns:
          - key: name
            title: NAME
            render:
              type: text
              path: metadata.name
```

## 2. 渲染规则

### 2.1 菜单

- 一级 `page` 菜单直接生成叶子菜单
- 一级 `organization` 菜单先生成一个分组菜单
- 一级 `organization` 菜单的 `name` 仍然使用菜单生成规则
- 二级页面菜单的 `parent` 指向一级组织菜单的 `name`

### 2.2 路由

- 一级页面后缀：`<first-key>`
- 二级页面后缀：`<first-key>/<second-key>`

最终路径：

- `cluster`: `/clusters/:cluster/frontendintegrations/<fi-name>/<suffix>`
- `workspace`: `/workspaces/:workspace/frontendintegrations/<fi-name>/<suffix>`
- `global`: `/frontendintegrations/<fi-name>/<suffix>`

### 2.3 pageId

规则：

```text
<fi-name>-<placement>-<suffix-slug>
```

示例：

- `demo-fi-cluster-overview`
- `demo-fi-workspace-ops-inspecttasks`

## 3. 输出 Manifest 示例

```json
{
  "version": "1.0",
  "name": "demo-fi",
  "displayName": "Demo FI",
  "description": "Demo multi-page integration",
  "routes": [
    {
      "path": "/clusters/:cluster/frontendintegrations/demo-fi/overview",
      "pageId": "demo-fi-cluster-overview"
    },
    {
      "path": "/workspaces/:workspace/frontendintegrations/demo-fi/ops/inspecttasks",
      "pageId": "demo-fi-workspace-ops-inspecttasks"
    }
  ],
  "menus": [
    {
      "parent": "cluster",
      "name": "frontendintegrations/demo-fi/overview",
      "title": "Overview",
      "icon": "GridDuotone",
      "order": 999
    },
    {
      "parent": "workspace",
      "name": "frontendintegrations/demo-fi/ops",
      "title": "Ops",
      "icon": "GridDuotone",
      "order": 999
    },
    {
      "parent": "frontendintegrations/demo-fi/ops",
      "name": "frontendintegrations/demo-fi/ops/inspecttasks",
      "title": "Inspect Tasks",
      "icon": "GridDuotone",
      "order": 999
    }
  ],
  "locales": [],
  "pages": [
    {
      "id": "demo-fi-cluster-overview",
      "entryComponent": "demo-fi-cluster-overview",
      "componentsTree": {
        "meta": {
          "id": "demo-fi-cluster-overview",
          "name": "demo-fi-cluster-overview",
          "title": "Overview",
          "path": "/demo-fi-cluster-overview"
        },
        "context": {},
        "root": {
          "id": "demo-fi-cluster-overview-root",
          "type": "Iframe",
          "props": {
            "FRAME_URL": "http://example.test/frontend"
          },
          "meta": { "title": "Iframe", "scope": true }
        }
      }
    },
    {
      "id": "demo-fi-workspace-ops-inspecttasks",
      "entryComponent": "demo-fi-workspace-ops-inspecttasks",
      "componentsTree": {
        "meta": {
          "id": "demo-fi-workspace-ops-inspecttasks",
          "name": "demo-fi-workspace-ops-inspecttasks",
          "title": "Inspect Tasks",
          "path": "/demo-fi-workspace-ops-inspecttasks"
        },
        "context": {},
        "dataSources": [
          {
            "id": "columns",
            "type": "crd-columns",
            "config": {
              "COLUMNS_CONFIG": [
                {
                  "key": "name",
                  "title": "NAME",
                  "render": {
                    "type": "text",
                    "path": "metadata.name",
                    "payload": {}
                  }
                }
              ],
              "HOOK_NAME": "useCrdColumns"
            }
          },
          {
            "id": "pageState",
            "type": "workspace-crd-page-state",
            "args": [
              { "type": "binding", "source": "columns", "bind": "columns" }
            ],
            "config": {
              "PAGE_ID": "demo-fi-workspace-ops-inspecttasks",
              "CRD_CONFIG": {
                "apiVersion": "v1alpha2",
                "kind": "InspectTask",
                "plural": "inspecttasks",
                "group": "kubeeye.kubesphere.io",
                "kapi": true
              },
              "HOOK_NAME": "useCrdPageState"
            }
          }
        ],
        "root": {
          "id": "demo-fi-workspace-ops-inspecttasks-root",
          "type": "CrdTable",
          "props": {
            "TABLE_KEY": "demo-fi-workspace-ops-inspecttasks",
            "TITLE": "Inspect Tasks"
          },
          "meta": { "title": "CrdTable", "scope": true }
        }
      }
    }
  ],
  "build": {
    "target": "kubesphere-extension",
    "moduleName": "demo-fi",
    "systemjs": true
  }
}
```

## 4. 说明

- 顶层 `displayName` 取 `spec.displayName`，缺省回退 `metadata.name`
- 页面标题来自绑定菜单节点的 `displayName`
- `global` 在产品语义中对应“扩展坞”
- `crdTable` 页面在 `workspace` placement 下继续使用 `workspace-crd-page-state`
