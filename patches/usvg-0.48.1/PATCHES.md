# usvg 0.48.1 补丁登记（PATCHES.md — usvg 卷）

> usvg 0.48.1（crates.io 逐字节副本）本地化在 `.3rd/usvg-0.48.1`，由 `main/Cargo.toml`
> 的 `[patch.crates-io]` 接进依赖树。与 windui 卷（`patches/PATCHES.md`）同一条政策：
> 对它的任何修改必须在本登记簿留章节，并在被改文件用 `★ RuPix 补丁 Uxx` 标注；
> 归档（本目录 `src/`）与工作副本逐字节一致由 `docs_consistency::j13` 硬拦。
> 升级 resvg/usvg 时整份重放；两份许可（Apache-2.0 / MIT）随副本携带。

---

## 补丁 U02 —— face 数据线程级缓存（根治 fontdb `Source::File` 的每次整读）

- 编号：U02
- 涉及上游文件：`usvg-0.48.1/src/text/layout.rs`
  （新增 `pub(crate) fn with_face_data_cached` + `DatabaseExt`/`shape_text_with_font`
  共 8 处调用点改走它；本卷目前唯一补丁，编号从 U02 起是因为 U01——`ShaperData`
  缓存——曾落地后被读数判废撤除，编号不复用）
- 应用侧接线：无（纯 usvg 内部；windui/宿主零改动）。

### 问题（RuPix m185 实测）
带 CJK 文字的 SVG 完整解析要 400–1600 ms，且**每份文档重付**：同进程字体全热、
同族、第二份文档照样 1400+ ms。判别测量锁定：Arial 纯 ASCII 6 节点 40 ms，
等线纯 ASCII 同等内容 714 ms——成本按**字体文件大小 × 字形访问次数**计价。

### 根因
fontdb 未开 `memmap` feature 时，`Source::File` 的 `with_data` 是
**每次调用都 `std::fs::read` 整个字体文件**。usvg 的文字路径（`load_font` /
`has_char` / shaping / 逐字形轮廓提取）全部走 `fontdb::with_face_data`，且上游
**没有任何一层缓存**——一个 6 节点中文文档要碰同一份 20 MB 字体 80+ 次。

### 修复
新增 `with_face_data_cached(db, id, p)`：按 face 缓存 `(数据, face_index)`
（thread_local，CAP 32 满即清），首次 `fs::read` 一次、之后纯内存；`Binary` 路径
不复制直接借。8 处调用点改走它。`ShaperData`/渲染语义一字未动。

### 验收（可复现）
- **输出逐字节一致**：10 份文档（含 图片1.svg、等线/Arial/回退混排/无文字）
  补丁前后像素指纹全部相同（`examples/svg_font_cost_probe` 的 fp 列）。
- **耗时**（warm 臂，同进程）：图片1.svg 767 → **31 ms**；等线 6 节点 545 → **4 ms**；
  等线 1 节点 407 → 1 ms；Arial+CJK 混排 748 → 45 ms（残留 = 逐字回退扫描，
  另账）；无文字 0 ms。
- usvg 自带测试全绿（`cargo test --release`，在其目录内跑）。

### 门禁
升级 usvg 时：先看上游是否已把 `Source::File` 改成缓存/共享映射（改了则本补丁
整段撤掉）；没有就按原样重放。`j13`：`patches/usvg-0.48.1/src/text/layout.rs`
与 `.3rd/usvg-0.48.1/src/text/layout.rs` 逐字节相同。

---

## 判废记录

- **U01（`ShaperData` 线程级缓存）——判废撤除（2026-11-01）**。假设"每 text chunk
  重建排版器是大头"，落地后实测时间不降反持平（指纹一致）⇒ U02 揭示真实大头是
  重复文件 I/O；U02 落地后 6 节点文档全趟 4 ms，`ShaperData::new` 的残账不足 1 ms/次。
  按"最小改动"撤除，编号不复用。
