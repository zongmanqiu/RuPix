# tests/ —— 集成测试（门禁）与真机探针（按需）

双层隔离，对齐 CurSeen：

| 区域 | 内容 | 默认 `cargo test` | 说明 |
|---|---|---|---|
| `tests/*.rs`（根） | 集成测试 = **门禁**，零 `#[ignore]` | 全跑 | `docs_consistency.rs`（文档/仓库门禁）、`config_compat.rs`（配置兼容）、`config_pipeline_e2e.rs`（配置→解码管线端到端：真实 DLL + ini 键）、`locked_decoder_dll.rs`（真机锁：正被加载的解码器 DLL 删不掉 / 淘汰不卸库 / 坏新版不打没格式）等 |
| `tests/probes/*.rs` | 真机探针（真 DLL / 真解码路径 / 真文件系统） | **完全不编译** | 机制核心：cargo 只自动发现 `tests/*.rs`，**不递归子目录**；不显式登记 `[[test]]` 就零编译成本 |
| `examples/*.rs` | 离屏探针（`fn main`，进程内判据） | 不参与 | 开发期专用，不进发布 |

- 单元测试仍内联在 `src/**` 的 `#[cfg(test)]` 块里（惯例：**生产代码在前、mod tests 在文件末尾**，
  这是 `hygiene_scan.py` 的 cfg(test) 豁免所依赖的硬性布局）。
- 探针写法模板（头部注释四件套：一句话定性 / 背景动机 / 判据 / 跑法）见 `probes/README.md`。
- 跑门禁：`main\build\5_test.bat`（或 `python build/gate.py run <组>` 按需）。
