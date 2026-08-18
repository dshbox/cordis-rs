# cordis-cli

[English](README.md) | **简体中文**

[cordis-rs](https://crates.io/crates/cordis-rs) 插件框架的命令行运行器：
`cordis run <config.yml>`。

在 [cordis-loader](https://crates.io/crates/cordis-loader) 之上，为配置驱动的
cordis 应用提供一个进程模型：

```console
$ cordis run cordis.yml
cordis: worker ready (2 entries, config: cordis.yml)
```

- **daemon / worker** —— `cordis run` 监督一个启动 loader 的 worker 子进
  程。worker 以退出码 `51` 请求热重启、`52` 表示退出、`53` 表示 loader
  根本没起来；只有 `51` 会触发重新拉起。
- **信号** —— `SIGINT` / `SIGTERM` 会优雅销毁根 context 并以 `52` 退出，
  因此 Ctrl+C 能干净地停掉整个应用。只送达 daemon 的信号
  （`kill <pid>`、按进程单独下杀的 supervisor）也会被转发：daemon 关闭
  worker 的 stdin 管道，worker 走与信号相同的优雅关停路径；十秒内不退出
  的 worker 会被强杀——因此 daemon 因任何原因死亡时 worker 都会跟着退出。
- **dotenv** —— 启动时从工作目录加载 `.env` 和 `.env.local`（已有的环境
  变量始终优先），配置中的 `${{ env.NAME }}` 模板由此展开。
- **热重载** —— 条目文件被监视（防抖）；外部编辑通过 loader 的 diff
  机制协调到 fiber。

插件通过 `worker` 服务停止或重启进程：

```rust,ignore
// 在插件的 apply 内，`ctx` 为插件 context：
let handle = ctx.require::<cordis_cli::worker::WorkerHandle>("worker")?;
handle.restart(); // 整个 worker 重载（退出码 51）
```

## 范围

原生二进制只注册内置的 `group` 插件；其他名字的条目会被记录到 loader 的
`last_error()` 并跳过，除非通过 `--plugin-dir <dir>`（可重复）让它们从
动态库插件解析——加载前会按运行中工具链做指纹校验，库一变更就热重启
worker。嵌入方应用通过
[`cordis_loader::PluginRegistry`](https://docs.rs/cordis-loader) 注册自己的
插件，或 fork 本运行器。
