# FrontendIntegration CRD 定义

## 1. 设计目标

`FrontendIntegration` CRD 用于声明前端扩展的用户意图。

当前版本的 `spec` 被拆成两个区块：

1. `menus`：两级菜单树
2. `pages`：页面配置列表

一个 FI 可以声明多个页面入口，并通过菜单 `key` 和页面 `key` 建立 1:1 绑定。

## 2. 基本信息

```yaml
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: frontendintegrations.frontend-forge.kubesphere.io
spec:
  group: frontend-forge.kubesphere.io
  scope: Cluster
  names:
    plural: frontendintegrations
    singular: frontendintegration
    kind: FrontendIntegration
    shortNames:
      - fi
```

## 3. Spec 结构

### 3.1 顶层结构

```yaml
spec:
  displayName: Demo FI
  enabled: true
  menus: []
  pages: []
  builder:
    engineVersion: v1
```

字段说明：

- `displayName`：扩展显示名称，可选
- `enabled`：是否启用，可选，默认 `true`
- `menus`：菜单树，必填
- `pages`：页面配置，必填
- `builder.engineVersion`：runner 使用的 manifest 渲染版本，可选

## 4. 菜单区块

### 4.1 一级菜单

```yaml
menus:
  - displayName: 概览
    key: overview
    placement: cluster
    type: page

  - displayName: 运维
    key: ops
    placement: workspace
    type: organization
    children:
      - displayName: 检查任务
        key: inspecttasks
      - displayName: 检查规则
        key: inspectrules
```

一级菜单字段：

- `displayName`
- `key`
- `placement`
- `type`
- `children`，仅 `organization` 使用

`placement` 枚举：

- `cluster`
- `workspace`
- `global`

说明：

- `global` 对应产品语义中的“扩展坞”
- 一级菜单 `placement` 是单值，不再支持旧的多 placement 数组

### 4.2 二级菜单

二级菜单字段只有：

- `displayName`
- `key`

二级菜单默认是页面节点，并继承所属一级菜单的 `placement`。

### 4.3 菜单约束

- 菜单只支持两级
- `type=page` 时，`children` 必须为空
- `type=organization` 时，`children` 必须非空
- 一级组织菜单自身不能绑定页面配置

## 5. 页面区块

### 5.1 页面列表

```yaml
pages:
  - key: overview
    type: iframe
    iframe:
      src: http://example.test/frontend

  - key: inspecttasks
    type: crdTable
    crdTable:
      names:
        kind: InspectTask
        plural: inspecttasks
      group: kubeeye.kubesphere.io
      version: v1alpha2
      scope: Cluster
      columns:
        - key: name
          title: NAME
          render:
            type: text
            path: metadata.name
```

公共字段：

- `key`
- `type`

页面类型：

- `iframe`
- `crdTable`

### 5.2 iframe 页面

```yaml
pages:
  - key: overview
    type: iframe
    iframe:
      src: http://example.test/frontend
```

字段说明：

- `iframe.src`：页面地址
- 兼容 `url` 作为 `src` 的别名

### 5.3 crdTable 页面

```yaml
pages:
  - key: inspecttasks
    type: crdTable
    crdTable:
      names:
        kind: InspectTask
        plural: inspecttasks
      group: kubeeye.kubesphere.io
      version: v1alpha2
      authKey: kubeeye-auth
      scope: Cluster
      columns:
        - key: name
          title: NAME
          render:
            type: text
            path: metadata.name
```

字段说明：

- `names.kind`
- `names.plural`
- `group`
- `version`
- `authKey`
- `scope`
- `columns`

其中：

- `scope` 枚举值：`Namespaced | Cluster`
- `columns` 是唯一列配置来源

## 6. key 与绑定规则

### 6.1 key 格式

所有菜单 key 和页面 key 均使用 kebab-case 路由片段：

```text
^[a-z0-9]([a-z0-9-]*[a-z0-9])?$
```

### 6.2 唯一性

- 一级菜单 `key` 在一级菜单范围内唯一
- 所有页面型菜单 key 全局唯一
- `pages[].key` 在页面列表内唯一

### 6.3 绑定

- 一级 `type=page` 菜单，必须绑定同名 `pages[].key`
- 一级 `type=organization` 菜单的每个二级菜单，必须绑定同名 `pages[].key`
- 每个 `pages[].key` 必须且只能被一个菜单节点引用

## 7. 路由、菜单名与页面 ID 派生

### 7.1 路由后缀

- 一级页面：`<first-key>`
- 二级页面：`<first-key>/<second-key>`

### 7.2 菜单 name

- 一级页面：`frontendintegrations/<fi-name>/<first-key>`
- 一级组织：`frontendintegrations/<fi-name>/<org-key>`
- 二级页面：`frontendintegrations/<fi-name>/<first-key>/<second-key>`

### 7.3 路由 path

- `cluster`: `/clusters/:cluster/frontendintegrations/<fi-name>/<suffix>`
- `workspace`: `/workspaces/:workspace/frontendintegrations/<fi-name>/<suffix>`
- `global`: `/frontendintegrations/<fi-name>/<suffix>`

### 7.4 pageId

```text
<fi-name>-<placement>-<suffix-slug>
```

其中 `suffix-slug` 为路由后缀中的 `/` 替换成 `-`。

## 8. 示例 CR

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
      placement: global
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
          kind: InspectTask
          plural: inspecttasks
        group: kubeeye.kubesphere.io
        version: v1alpha2
        scope: Cluster
        columns:
          - key: name
            title: NAME
            render:
              type: text
              path: metadata.name
  builder:
    engineVersion: v1
```

## 9. 总结

新的 `FrontendIntegration.spec` 不再表达“单个集成 + 单条路由 + 单个菜单入口”，而是表达：

- 一个菜单树
- 一组页面定义
- 菜单与页面之间明确的 key 绑定关系
