# Frontend Forge Controller

`Frontend Forge Controller` 是一个围绕 `FrontendIntegration` 自定义资源构建的 Kubernetes 控制器，用来把前端扩展的声明式配置收敛成可交付的 `JSBundle` 产物。

项目当前的核心目标是：

- 让用户通过一个 cluster-scoped 的 `FrontendIntegration` CR 描述前端入口
- 由 controller 负责幂等、状态维护和 Job 调度
- 由 runner 负责把 `FrontendIntegration.spec` 渲染成 Manifest，并调用外部 build-service 构建前端产物

相关设计见 [`spec/design.md`](spec/design.md)。

## 当前架构

当前实现由三部分组成：

- `FrontendIntegration`：用户入口 CR，表达菜单、页面和构建引擎版本等意图
- `frontend-forge-controller`：监听 `FrontendIntegration` 和 `Job`，负责状态流转、Job 创建、失败处理和 `JSBundle` 关联
- `frontend-forge-runner`：作为一次性 Job 运行，读取 `FrontendIntegration`，渲染 Manifest，调用 build-service，并写回产物与状态

资源关系大致如下：

1. 用户提交 `FrontendIntegration`
2. controller 基于 `spec_hash` 判断是否需要发起构建
3. controller 创建 runner Job
4. runner 渲染 Manifest，并调用外部 build-service
5. runner 更新 `JSBundle`、ConfigMap 以及 `FrontendIntegration.status`

## 现有功能

### FrontendIntegration 模型

- 提供 `frontend-forge.kubesphere.io/v1alpha1` 的 `FrontendIntegration` CRD
- `FrontendIntegration` 为 cluster-scoped 资源，短名为 `fi`
- 当前 `spec` 支持：
  - `displayName`
  - `enabled`
  - `menus`
  - `pages`
  - `builder.engineVersion`
- `menus` 支持两级结构：
  - 一级 `type=page`
  - 一级 `type=organization` + 二级页面菜单
- `placement` 支持 `global`、`workspace`、`cluster`

### 页面与 Manifest 渲染

- 支持两类页面：
  - `iframe`
  - `crdTable`
- 支持 `menus[].key` 与 `pages[].key` 的 1:1 绑定
- runner 会在构建前执行语义校验，包括：
  - 重复菜单 key
  - 重复页面 key
  - 页面配置缺失
  - 孤儿页面配置
  - 非法菜单结构
  - 非法页面结构
  - 不支持的 `builder.engineVersion`
- 当前 Manifest 渲染器基于 `v1` 引擎实现

### 构建与状态管理

- controller 基于 `spec_hash` 做幂等判断和 Job 复用
- runner 基于渲染结果计算 `manifest_hash` 做构建追溯
- `enabled` 不参与 `spec_hash`，支持停用/启用时复用同一份规格身份
- controller 会维护 `FrontendIntegration.status`，包括：
  - `phase`
  - `last_build`
  - `bundle_ref`
  - `message`
  - `last_error`
- runner 失败时会把真实错误回写到 `status.message` 和 `status.last_error`
- controller 会尽量保留 runner 写入的业务错误，而不是只显示 `Job has reached the specified backoff limit`

### 运行与交付

- 提供 controller 与 runner 的 Dockerfile
- 提供基础部署 YAML：
  - [`config/manager/controller-deployment.yaml`](config/manager/controller-deployment.yaml)
  - [`config/rbac/controller-rbac.yaml`](config/rbac/controller-rbac.yaml)
  - [`config/rbac/runner-rbac.yaml`](config/rbac/runner-rbac.yaml)
- 提供示例 `FrontendIntegration` 清单：
  - [`config/samples/frontend-forge_v1alpha1_frontendintegration.yaml`](config/samples/frontend-forge_v1alpha1_frontendintegration.yaml)
  - [`config/samples/fi-inspecttask.yaml`](config/samples/fi-inspecttask.yaml)
  - [`config/samples/fi-nested-menu-demo.yaml`](config/samples/fi-nested-menu-demo.yaml)

## 当前限制

- `FrontendIntegration` 的语义校验仍发生在 runner Job 内，而不是 admission 阶段
- 当前依赖外部 build-service，仓库本身不包含前端构建服务实现
- Manifest 渲染引擎目前只有 `v1`
- 部署方式当前以原生 YAML 为主，未提供 Helm chart 或 Kustomize 方案

## TODO

- 抽出共享的 `frontend-forge-manifest` crate，统一承载 Manifest 渲染与语义校验逻辑
- 实现 controller 内 validating admission webhook，在 `CREATE` / `UPDATE` 时前置校验 `FrontendIntegration`
- 使用 `axum` 承载 webhook HTTP 服务
- 使用 [`kube-webhook-certgen`](https://github.com/jet/kube-webhook-certgen) 管理 webhook 证书和 `caBundle`
- 在不新增独立 validator Deployment 的前提下，把 webhook 集成进现有 controller Deployment

说明：

- 以上 TODO 目前仅完成设计，不属于当前实现
- 详细规划见 [`spec/design.md`](spec/design.md) 的 `vNext: Admission Webhook 前置校验`

## 技术栈

- Rust 2024 edition
- Tokio
- `kube` / `kube-runtime`
- `k8s-openapi`
- Serde / `serde_json` / `serde_yaml`
- Snafu
- Tracing / `tracing-subscriber`
- Reqwest
- Docker 多阶段构建 + distroless runtime

## 仓库结构

- [`crates/api`](crates/api)：CRD 类型定义、状态结构、CRD 导出
- [`crates/common`](crates/common)：通用常量、hash 和命名工具
- [`crates/controller`](crates/controller)：controller 主逻辑
- [`crates/runner`](crates/runner)：runner Job 逻辑与 Manifest 渲染
- [`xtask`](xtask)：开发辅助命令，如生成 CRD
- [`config`](config)：部署、RBAC、CRD 和样例清单
- [`spec`](spec)：设计文档

## 开发

常用命令：

```bash
cargo test --workspace
cargo xtask gen-crd
```

构建镜像对应的二进制：

```bash
cargo build --release -p frontend-forge-controller
cargo build --release -p frontend-forge-runner
```

Git hooks：

- `lefthook install`
- `pre-commit` 会重新生成 CRD
- `pre-push` 会校验 CRD 是否与代码一致

## 外部依赖

当前运行时默认依赖一个可访问的 build-service，controller 会把其地址通过 `BUILD_SERVICE_BASE_URL` 传递给 runner。

默认值见 [`config/manager/controller-deployment.yaml`](config/manager/controller-deployment.yaml)：

```yaml
env:
  - name: BUILD_SERVICE_BASE_URL
    value: http://frontend-forge.extension-frontend-forge.svc
```
