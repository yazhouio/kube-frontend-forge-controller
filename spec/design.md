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

## 9. vNext: Admission Webhook 前置校验

本章节记录下一版本的规划设计，不代表当前实现。

### 9.1 当前问题

当前 `FrontendIntegration` 的语义校验发生在 runner Job 内部：

1. 用户先创建或修改 `FrontendIntegration`
2. Controller 观察到变更后创建 build Job
3. runner 在 Job 中读取 FI，并执行 Manifest 渲染与校验
4. 若校验失败，最终表现为 Job 失败

这会带来两个问题：

- 错误暴露过晚。用户提交成功后，必须等到 Job 运行失败才能看到问题。
- Kubernetes Job 层面的失败信息经常被通用状态覆盖，例如 `Job has reached the specified backoff limit`，真实业务错误不够直接。

一个典型例子是重复页面 key：

- 当前行为：FI 已创建成功，runner 运行后报错 `FrontendIntegration sssxxx has duplicate page key 'frontendintegrations'`
- vNext 目标：在 admission 阶段直接拒绝 `CREATE` 或 `UPDATE`，请求返回原始校验错误，不再先创建资源再等 Job 失败

### 9.2 目标行为

下一版本将把 `FrontendIntegration` 的语义校验前移到 Kubernetes admission 阶段。

目标行为如下：

- 仅对 `FrontendIntegration` 的 `CREATE` 和 `UPDATE` 做前置校验
- 校验通过，API Server 正常持久化对象，后续流程保持现状
- 校验失败，API Server 直接拒绝请求，返回共享校验逻辑的原始错误消息
- 当前 runner 内部仍保留同一套校验逻辑，作为防御性兜底，而不是主入口

### 9.3 架构方案

下一版本采用“单 Deployment + 抽共享 crate + controller 内 webhook”的方案。

#### 共享校验层

新增共享 crate：`frontend-forge-manifest`

职责：

- 暴露 `render_extension_manifest(&FrontendIntegration) -> Result<Value, ManifestRenderError>`
- 暴露 `validate_frontend_integration(&FrontendIntegration) -> Result<(), ManifestRenderError>`

这层承载当前 runner 中的 Manifest 渲染和语义校验逻辑，避免把同一份规则复制到 controller 中。

设计原因：

- 不把 runner 规则硬拷贝到 controller，避免双份实现漂移
- runner 与 admission webhook 复用同一套领域逻辑，规则变更只有一个来源
- controller 只接入共享 crate，不负责维护另一套独立转换规则

#### Controller 内 webhook

Webhook 不单独起 validator Deployment，而是运行在现有 controller 进程内。

HTTP 实现固定为 `axum`，暴露两个接口：

- `GET /healthz`
- `POST /validate/frontendintegrations`

其中：

- `POST /validate/frontendintegrations` 接收 `AdmissionReview`
- 仅处理 `CREATE` 和 `UPDATE`
- 请求中的 `FrontendIntegration` 对象会交给 `frontend-forge-manifest::validate_frontend_integration`
- 若校验失败，则直接返回拒绝响应，并保留原始错误消息

选择 controller 内 webhook，而不是独立 validator Deployment，原因是：

- 当前项目规模下，单 Deployment 更容易交付和维护
- 不额外增加常驻服务、ServiceAccount、发布和运维成本
- 通过共享 crate 复用逻辑后，controller 进程内接 webhook 的耦合是可接受的

### 9.4 部署与启用方式

下一版本仍复用现有 `frontend-forge-controller` Deployment，不新增独立 validator Deployment。

Deployment 预期新增的 webhook 相关能力：

- webhook 监听端口
- `WEBHOOK_ENABLED`
- `WEBHOOK_BIND_ADDR`
- `WEBHOOK_CERT_PATH`
- `WEBHOOK_KEY_PATH`
- webhook 证书 Secret mount

默认策略：

- webhook 默认关闭
- 不作为当前安装清单的默认启用路径

这样做的原因是：

- 避免现有环境在未准备证书和 webhook 配置时直接启动失败
- 保持当前 controller 部署行为不变
- 让 webhook 作为显式开启的增强能力逐步接入

### 9.5 Webhook 规则

下一版本的 `ValidatingWebhookConfiguration` 设计固定如下：

- resource: `frontendintegrations`
- apiGroup: `frontend-forge.kubesphere.io`
- apiVersion: `v1alpha1`
- scope: `Cluster`
- operations: `CREATE`, `UPDATE`
- failurePolicy: `Fail`
- sideEffects: `None`

校验失败时，返回值直接透传共享校验层的业务错误，不再降级成 Job 级别的通用报错。

### 9.6 证书管理方案

下一版本的主方案使用 [`kube-webhook-certgen`](https://github.com/jet/kube-webhook-certgen) 管理 webhook 证书，而不是依赖 `cert-manager`。

设计前提：

- 当前仓库以原生 YAML 为主，没有 Helm hook 或 Kustomize 证书注入体系
- 当前目标环境不把 `cert-manager` 作为默认依赖

`kube-webhook-certgen` 在该方案中的职责：

- 生成或刷新 webhook 使用的 TLS Secret
- 把 CA 注入 `ValidatingWebhookConfiguration.caBundle`

因此，下一版本预期新增的对象除了 webhook 本身，还包括 certgen 相关资源：

- certgen Job
- certgen 所需的 ServiceAccount / RBAC
- controller webhook 使用的 TLS Secret
- controller 对应的 Service
- `ValidatingWebhookConfiguration`

由于仓库当前采用原生 YAML 组织方式，下一版本会以额外 Job/Service/WebhookConfiguration 清单的形式接入 `kube-webhook-certgen`，而不是依赖 Helm hook。

### 9.7 请求流

vNext 目标流程如下：

1. 用户提交 `FrontendIntegration`
2. API Server 调用 validating webhook
3. webhook 使用共享 crate 做语义校验
4. 校验通过，则对象持久化，controller 按现有逻辑继续 reconcile
5. 校验失败，则请求直接被拒绝，不创建新的 build Job

### 9.8 非目标

以下内容不在下一版本该方案的范围内：

- 本版本直接实现 admission webhook
- 新增独立 validator Deployment
- 引入 mutating webhook
- 把 runner 逻辑复制一份到 controller

### 9.9 示例

若用户提交如下存在重复页面 key 的 FI 变更：

- `pages[0].key = frontendintegrations`
- `pages[1].key = frontendintegrations`

当前行为：

- 对象会先写入 API Server
- controller 创建 build Job
- runner 在 Job 内失败
- 用户往往先看到 `Job has reached the specified backoff limit`，再需要继续追查日志

vNext 目标行为：

- API Server 在 admission 阶段直接拒绝请求
- 拒绝消息直接返回类似：
  `FrontendIntegration sssxxx has duplicate page key 'frontendintegrations'`
- 不会创建新的 build Job

### 9.10 结论

下一版本的 admission 校验方案固定为：

- 复用现有 controller Deployment
- 使用 `axum` 在 controller 内提供 validating webhook
- 抽 `frontend-forge-manifest` 作为共享校验与渲染层
- 使用 `kube-webhook-certgen` 管理 webhook 证书
- 默认关闭 webhook，避免影响现有部署

该方案的核心目标是把 FI 语义错误从“构建时失败”前移到“提交时拒绝”，让错误反馈更早、更准确，同时不引入额外常驻 Deployment。
