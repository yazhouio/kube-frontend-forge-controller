# Frontend Forge Builder Controller 设计文档

## 1. 当前模型

当前实现以 `FrontendIntegration`（FI）作为唯一用户入口，`Job` 作为一次性 runner，`JSBundle` 作为产物 CR。

核心收敛点：

1. Controller 不渲染 Manifest，只基于 `FI.spec` 做幂等和状态维护。
2. Runner 在 Job 内部读取 FI，并按 `spec.builder.engineVersion` 把 FI 转换成 Manifest。
3. `FI.spec` 已切换为双区块模型：
   - `spec.menus`：两级菜单树
   - `spec.pages`：页面配置列表
4. 页面型菜单通过 `key` 和 `pages[].key` 建立 1:1 绑定。

## 2. FrontendIntegration 语义

FI 是 cluster-scoped CR，表达“用户想要什么前端扩展”，而不是“构建过程如何执行”。

当前关键字段：

- `spec.displayName`
- `spec.enabled`
- `spec.menus`
- `spec.pages`
- `spec.builder.engineVersion`

其中：

- 一级菜单支持 `type=page | organization`
- 二级菜单默认就是页面节点
- 一级菜单 `placement` 为单值，枚举为 `cluster | workspace | global`
- `global` 在产品文档中对应“扩展坞”

## 3. 菜单与页面绑定规则

### 3.1 菜单树

- 菜单只支持两级
- `type=page` 的一级菜单不能包含 `children`
- `type=organization` 的一级菜单必须包含至少一个子菜单
- 组织菜单自身不对应页面配置
- 二级菜单继承一级菜单的 `placement`

### 3.2 key 约束

- 一级菜单 `key` 在一级范围内唯一
- 所有页面型菜单 key 全局唯一
- `pages[].key` 必须和页面型菜单 key 1:1 对应
- key 统一使用 kebab-case 路由片段

### 3.3 路由派生

- 一级页面菜单路由后缀：`<first-key>`
- 二级页面菜单路由后缀：`<first-key>/<second-key>`

最终路由：

- `cluster`: `/clusters/:cluster/frontendintegrations/<fi-name>/<suffix>`
- `workspace`: `/workspaces/:workspace/frontendintegrations/<fi-name>/<suffix>`
- `global`: `/frontendintegrations/<fi-name>/<suffix>`

## 4. Manifest 派生

Runner 渲染 Manifest 时按如下规则工作：

1. 先校验 `spec.menus` 和 `spec.pages`
2. 解析出组织菜单节点和叶子页面节点
3. 为每个页面节点生成：
   - `routes[]`
   - `menus[]`
   - `pages[]`
4. 为每个组织菜单生成一个分组 `menus[]`

页面 id 生成规则：

- `<fi-name>-<placement>-<suffix-slug>`
- `suffix-slug` 由路由后缀中的 `/` 替换成 `-`

示例：

- `overview` + `cluster` -> `demo-fi-cluster-overview`
- `ops/inspecttasks` + `workspace` -> `demo-fi-workspace-ops-inspecttasks`

## 5. 页面类型

### 5.1 iframe

- `pages[].type=iframe`
- 绑定 `pages[].iframe.src`
- 渲染成 `Iframe` 页面节点

### 5.2 crdTable

- `pages[].type=crdTable`
- 绑定 `pages[].crdTable`
- `columns` 仅从 `pages[].crdTable.columns` 读取

placement 的页面状态行为保持现状：

- `workspace` -> `workspace-crd-page-state`
- `cluster/global` -> `crd-page-state`

## 6. 构建与幂等

当前实现保留双 hash 模型：

- `spec_hash = sha256(canonical_json(FI.spec))`
- `manifest_hash = sha256(canonical_json(rendered_manifest))`

职责分工：

- Controller 依赖 `spec_hash` 做幂等、Job 复用和状态判断
- Runner 依赖 `manifest_hash` 做构建追溯和 `JSBundle` 标注

`enabled` 仍然不参与 `spec_hash` 计算，便于停用/启用时复用同一份 spec 身份。

## 7. 资源关系

- `Job.ownerReference -> FrontendIntegration`
- `JSBundle` 为第三方 cluster-scoped CR，当前不设置 ownerReference 到 FI
- 产物 ConfigMap 可设置 `ownerReference -> FrontendIntegration`

## 8. 结论

`FrontendIntegration` 现在是一个“多菜单、多页面”的意图层 CR：

- 菜单树负责入口组织
- 页面配置负责页面实现
- Runner 负责把这两部分收敛成最终 Manifest
