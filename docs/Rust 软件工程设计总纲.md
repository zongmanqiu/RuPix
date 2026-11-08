# Rust 软件工程设计总纲（最终整合版）

> **元原理**：把工程约束从"人的纪律"提升为"编译器的事实"。
> 依赖方向写进 `Cargo.toml`，暴露面写进可见性，约束写进类型，并发安全写进所有权，规范写进 CI。
> **认知边界**：编译期能力止于**序列化边界 / 运行时输入边界 / 调度器边界**，边界外必须靠测试与可观测性补位。
> **方法论**：用类型表达约束，用模块隔离副作用，用错误表达失败，用测试与门禁保证演进。

**用法**：时间紧只读 §1 决策表与 §15 落地，其余是按需查阅的依据。全文任一判断都可回溯到机制而非惯例。

---

## 采纳状态（本仓把哪几条变成了机器事实）

> 这份宪章是**依据**，不是执行面——谁能执行它？下表逐条给出落点与验证者。
> **改本表就好，别把规则往宪章正文里抄**：规则的正本在 `AGENTS.md`（硬性）与
> `开发指南.md` §一/§二（流程），本表只做"宪章条款 ↔ 落点"的对账。
> 桌面版总纲（含 OS 环境边界、五层 crate、MVU、平台集成、发布与杀软那几章）在仓库外
> **外部《Rust 软件工程设计总纲》`**——外部参考，不复制进仓（1300 行的方法论进 `docs/`
> 就是第二正本）。2026-10-19 按它做了一轮机检，结论在本文件与 `HISTORY.md`。

| 宪章条款 | 本仓落点 | 谁验证 | 状态 |
|---|---|---|---|
| §0.1 架构边界物理化 | 五层方向 + 各层禁引表 | `build/layering_scan.py`（唯一机器真源） | ✅ 违规 0 |
| §0.1 规范写进 CI | 六步流水线，第②步**七道**静态门禁（[1/7] 分层、[2/7] 卫生、[3/7] fmt、[4/7] clippy、[5/7] 结构文档新鲜度、[6/7] `dump_sym.py --selftest`、[7/7] 注释断链） | `build/2_scan.bat` / `full.bat` | ✅ |
| §1 参数所有权（绝不 `&T` 再 clone） | `开发指南.md` §二对照表约束 + 评审 | 人工评审 | ✅ 惯例 |
| §1 `#[from]` 禁用于关键路径 | 错误 enum 用 `#[source]` + 显式 `map_err` 附路径信息 | **只有人工评审**——`hygiene_scan` 不看这个，别把它当机器门禁 | ✅ 惯例 |
| §7.5 生产禁 `unwrap`/`expect` | 红线豁免须写在违规行上方 | `hygiene_scan.py` 第 5 查 `panic_source` | ✅ 违规 0 |
| §7.6 日志只打一次 | 错误只传递，请求边界统一记一次 | 评审 + `docs/踩坑记录/踩坑记录.md` | ✅ 惯例 |
| §9 三消费者模型（用户消息不透 `Display`） | 领域错误 enum → 消息映射表；解码器错误文案中文外层 | 解码器仓 `check_docs` + 宿主测试 | ✅ |
| §11 VERSIONINFO（"无翻转"） | `build.rs` + `assets/version.rc.in`（版本源 = `Cargo.toml`） | `deploy.py` **RT_VERSION 门禁**（K70） | ✅ 2026-10-19 |
| §10 日志头必含版本/OS/DPI/会话/参数 | `platform::env_report` 会话头，崩溃报告读启动时冻结的那份 | `env_report` 单测 + `.build/temp/rupix-<日期>.log` 真机读数 | ✅ 2026-10-19 |
| §7.2 原子写 temp + fsync + replace | `core::config::save_to` | 落盘往返测试只覆盖"内容完整"；**fsync 本身没有判据**（断电不可测），靠代码审查 + 注释里那条 NTFS 理由 | ✅ 2026-10-19 |
| §16 clippy `-D warnings` 进 CI | `2_scan.bat` 第 [4/6] 步 | 门禁自身（K69：不带 `-D warnings` 就是假绿） | ✅ 2026-10-19 |
| §2.4 组合根要薄（bin 对测试不可见） | argv 解析搬进 `rupix_lib::cli` | `cli` 5 条单测 | ✅ 2026-10-19 |
| §2.2 时钟端口化（试金石 D′） | **只做防抖那一处**：`poll_at(c,&p,now)` | 防抖到期判据单测 | ✅ 局部；`Clock` trait 判废（core 30 处时间读，收益不成比例） |
| §0.2 三条失效边界的守卫工具 | 边界 parse 单测 + 快照对拍 + 迁移契约类测试 | `cargo test`（含元数据盲点单测） | ✅ 部分。`loom` / `cargo-fuzz` **没引入，也没做过成本收益测算**——那是"没评估"，不是"已裁决"，别当结论引用 |
| §16 单文件 ≤800 行 | — | — | 🚫 判废：7 个文件超线，纯机械重排换不来正确性 |
| §15 每个 `unsafe {}` 带 `// SAFETY:` | FFI 边界与 unsafe item 必须有"为什么"注释 | `hygiene_scan.py` 第 4 查 `boundary_doc` | ✅ 替代形态；73 处块级注释判废 |
| §6.2 有界 channel | 解码队列 `Mutex<VecDeque>` + 每 16ms 全量抽干 | — | 🚫 判废：单枚串行解码线程，积压受解码吞吐约束 |
| §6.7 `#[non_exhaustive]` 加在公共类型 | — | — | 🚫 判废：ABI 契约自 v5 起签名冻结（现行 v6 加法式），加了破解码器构造 |
| §10 导出诊断包（一键打包日志+dmp+配置） | — | — | ⏳ 待办（没有用户支持渠道之前不做） |

**读法**：🚫 的三行不是"没做"，是**算过之后决定不做**——依据与配对数字在
`HISTORY.md` 与 `docs/踩坑记录/踩坑记录.md`；下次有人提议补上，先看那张表，别重新发明。

---

## 0. 地基

### 0.1 约束的可靠性阶梯、四条原则与五个执行机制

三层可靠性依次递增：

```
文档约定    "请不要在 domain 里用 sqlx"      → 人员变动即腐化
运行时检查   启动时扫描 Cargo.toml            → 滞后，出问题已在生产
编译期事实   domain 的 Cargo.toml 里没有它    → 无法失效：架构不成立就编译不过
```

**Rust 与 Java/Go 的本质差异**：Java 的 package 可见性、Go 的 `internal/` 只是弱边界，物理可达性由 classpath 决定；Rust 的 crate 依赖是**构建图本身**——依赖不在 `Cargo.toml` 里，符号就不存在。物理不可达，不是风格问题。

| 不可妥协的原则 | 内容 |
|---|---|
| 编译期约束替代运行时约定 | 每写下"请注意不要……"，就问：能否交给编译器？不能，说明抽象层次选低了 |
| 架构边界必须物理化 | 能用 crate 边界就别退到 lint，能用 lint 就别退到文档 |
| 可替换性优先于可复用性 | 有第二个实现（含测试 fake）才抽 trait。新手问"能否复用"，老手问"能否换掉" |
| 约束先于抽象 | 先列举所有非法用法并逐个消灭，剩下的合法用法才谈抽象 |

五个执行机制（强度递减）：

| 机制 | 强制什么 | 违反形态 |
|---|---|---|
| crate 依赖图 | 架构方向 | 编译失败 |
| 可见性梯度（`pub` / `pub(crate)` / `pub(super)`） | API 表面 | 无法引用 |
| newtype / enum / typestate | 领域约束 | 非法值无法构造 |
| `Result` + `#[must_use]` | 失败路径 | 未处理即告警 |
| clippy + cargo-deny + CI 脚本 | 行为规范 | CI 红 |

### 0.2 编译期的三条失效边界（与守卫工具同构）

只知道"能用编译器"会过度设计；知道它在哪失效，才知道测试与可观测性布防在哪。**三组东西是同一现象的三种表述**：

| 编译期失效边界 | Typestate 崩塌线 | 专属守卫工具 |
|---|---|---|
| **序列化边界**：数据库行/字节流无类型信息 | 持久化崩塌 | 往返 `proptest` + `insta` 快照 + 迁移契约测试 |
| **运行时输入边界**：外部数据编译期不可知 | 反序列化崩塌 | 边界 parse 测试 + `cargo-fuzz` |
| **调度器边界**：编译器不懂任务交织，类型是单线程证据 | 并发参与崩塌 | `loom` + `tokio-console` + 取消/关停演练 |

**边界在哪，守卫就在哪。** 这条映射让"为什么这些测试必需"有了依据，而非凭经验列清单。**推论**：线上故障大多发生在三条边界之外（外部系统、脏数据、调度），所以 Rust 项目仍必须做可观测性——编译期保证的是"代码内部自洽"。

### 0.3 四条验收试金石

- **A（依赖方向）**：删掉 `infrastructure/` 与 `server/`，`domain` + `application` 仍能编译？
- **B（可测试性）**：测一条核心业务规则，需要启动 DB / HTTP 吗？
- **C（可替换性）**：换掉数据库，要改业务逻辑吗？
- **D（纯函数性）**：领域函数相同输入永远相同输出？

**D 专抓"隐形副作用"**——`Cargo.toml` 干净但领域已不可测：

```rust
// ❌ 隐式读全局时钟："还有 3 天到期"无法确定性测试
pub fn is_active(&self) -> bool { Utc::now() < self.expires_at }
// ✅ 时间是输入：任意时刻可测，零 mock
pub fn is_active_at(&self, now: DateTime<Utc>) -> bool { now < self.expires_at }
```

同理适用于随机数、隐式 ID 生成。**领域函数输出只由输入决定**——这是领域层免 mock 的根基。任一试金石不过，其他优化都是装饰。

### 0.4 深水区地图（全文增量所在）

各资料都点到名字但没讲透的十处：① 取消安全三问与"移出 select"；② 编译器不拦 `parking_lot` 持锁跨 await；③ `JoinHandle` 被 drop 不 abort；④ feature 并集合流；⑤ `impl Trait` 四限制；⑥ AFIT 与 `dyn` 的兼容性；⑦ `#[non_exhaustive]` 对内不加；⑧ 可观测性基数控制；⑨ 时钟与随机数是隐形副作用；⑩ 组合根类型擦除与泛型传染。其中 ① 取消安全 与 幂等性（§7.4）、优雅关停（§8.7）被统一视角「**部分进度**」（§8.4）串成同一结构；②③④ 则是三条各自独立的深水区。

---

## 1. 决策速查表（日常只用这张）

用法：**看到 X 选"默认"，除非出现"翻转信号"**。模式会被误用，信号不会。

| 场景 | 默认 | 翻转信号 |
|---|---|---|
| 公共 API 错误类型 | 具体 `enum` + `#[non_exhaustive]` | 无翻转：公共 API 不返回 `anyhow::Error` |
| 端口分发 | `Arc<dyn Trait + Send + Sync>` | 热路径小方法/编译时间痛 → 泛型；实现封闭 → enum 分发 |
| 状态机 | `enum` + by-value 转换 | 线性 + 编译期可知 + 状态 ≤5 + 不落盘 + 单任务 → typestate |
| 共享状态 | 消息传递（channel + 单 owner） | 只读 → `Arc<T>`；写多 → 分片；真共享 → 锁（先过五问） |
| 锁类型 | `std::sync::Mutex` | 临界区必须跨 `.await` → `tokio::sync::Mutex`（先问能否重构掉） |
| 测试替身 | 手写 fake | 纯交互断言（"必须调用 N 次"）→ `mockall` |
| 项目结构 | 单 crate | 强制边界/发布边界/团队边界/编译时间 → workspace |
| `select!` 分支 | 只放 cancel-safe 操作 | 非取消安全 → **移出 `select!`**，先等命令再执行 |
| newtype 的 `Deref` | 不实现 | 无翻转：`Deref` 只给智能指针 |
| newtype 的 `Borrow<T>` | 不实现 | 无翻转：除非 `Hash`/`Eq` 与 `T` 严格一致 |
| 参数所有权 | 需要存储收 `T`、只读收 `&T` | 无翻转：**绝不 `&T` 进来再 `.clone()`** |
| 要不要加 trait | 有第二个实现（含 fake）才抽 | 无翻转：可替换性优先于可复用性 |
| `#[from]` | 禁用，显式 `map_err` 附路径 | 仅上下文无关的便捷转换可用 |
| `#[non_exhaustive]` | 公共类型加 | **内部类型不加**（穷尽性是自己的重构安全网） |
| `async fn` in trait | 原生 AFIT（泛型注入，零开销） | 需要 `Box<dyn>` + `Send` → `async-trait` |
| `impl Trait` 返回 | 隐藏实现用 `-> impl Trait` | 调用方要存储具体类型 → 具名类型 / newtype |
| panic | 只用于逻辑错误/不变量破坏 | 用户输入/网络/文件不存在 → `Result` |
| 日志 | 只在请求边界打一次 | 无翻转：错误只传递，不层层记录 |
| 自动重试 | 先过幂等性再谈重试 | 非幂等写 → 幂等键 / 先落意图 / 状态前置校验 |
| 要不要分层 | 模块化单体 | 变更轴数量 × 变更概率低 → 不分 |

---

## 2. 五处分歧裁决（团队宪法）

各资料已高度收敛，真正分歧只有五处。团队落地时，这类"裁判缺失"正是争论来源。

| 分歧点 | 裁决 | 依据 |
|---|---|---|
| `domain` 能否依赖 `serde` | **允许**，用 `#[serde(transparent)]`，不派生业务逻辑；要 schema 独立演进才移出做 DTO | `serde` 是数据表示不是副作用能力；移出收益是解耦，代价是一层样板，团队 <5 人时移出是过度设计 |
| 端口默认泛型还是 `dyn` | **默认 `Arc<dyn>`**，热路径小方法才泛型 | 泛型编译时间随"实现数 × 用例数"组合膨胀；repo 级边界虚跳转在毫秒 I/O 前是噪音 |
| 状态机 `enum` 还是 typestate | **默认 `enum`** + 按值转换；"线性 + 编译期可知 + 状态 ≤5 + 不落盘 + 单任务"同时成立才 typestate | typestate 三条崩塌线（§4.3） |
| `#[from]` | **关键路径禁用**，显式 `map_err` 附路径信息 | 信息论问题：`From` 签名 `fn from(E) -> Self` 没有参数携带"从哪来" |
| 库内 `#![deny(warnings)]` | **禁止**，门禁位置在 CI 的 `-D warnings` | 新工具链新增 lint 会无故破坏下游构建——这不是风格问题，是 semver 破坏 |

---

## 3. 架构：依赖图即架构

### 3.1 四 crate 骨架与物理审计

```text
my-app/
├── Cargo.toml            # virtual manifest
└── crates/
    ├── domain/           # 实体、值对象、端口 trait、领域错误——零基础设施依赖
    ├── application/      # 用例编排——只依赖 domain
    ├── infrastructure/   # 端口实现——这里才出现 sqlx/reqwest/tokio
    └── server/           # 组合根——依赖一切并装配
```

依赖方向：`server → infrastructure → application → domain`，且 `server` 也可直接依赖 `application`（组合根直接装配用例）；`domain` 谁也不依赖。

**领域依赖白名单**：`thiserror` / `chrono` / `uuid` / `rust_decimal` / `serde` / `nutype`。
**禁止**：`sqlx` `axum` `reqwest` `tokio` `tracing` `anyhow` `config`，以及 `std::fs` / `std::net`。

CI 审计（这条脚本比任何架构文档都有用）：

```bash
for c in my-domain my-application; do
  cargo tree -p "$c" --edges normal | grep -E '^(sqlx|axum|reqwest|redis|tokio) ' \
    && { echo "架构违规：$c 引入基础设施依赖"; exit 1; }
done
# 还要 grep 源码：domain 里不许出现 use std::fs / Utc::now()
```

### 3.2 依赖净化的精确语义

禁止的是**驱动进程边界的框架**（副作用能力），不是"数据表示"库。比 `sqlx` 更常见的两个隐蔽污染源：**时钟**（`Utc::now()` 是隐式全局读，破坏试金石 D）与**随机数/ID 生成**。判定：领域代码出现 `Utc::now()` / `SystemTime::now()` / `rand::random()` / `Uuid::new_v4()`，一律端口化：

```rust
pub trait Clock: Send + Sync { fn now(&self) -> DateTime<Utc>; }
pub struct SystemClock;                  // 生产
pub struct FixedClock(DateTime<Utc>);    // 测试：时间冻结
```

成本很低，收益是把"超时、过期、幂等窗口、重试退避"从"只能靠集成测试 + sleep"变成**纯单元测试**。

### 3.3 端口 trait 六条规则

1. **按"调用方需要什么"定义**，不是"实现方能做什么"（不照搬 Postgres 的分页/事务能力）。
2. **错误类型领域定义**：`Result<_, RepoError>`，绝不泄漏 `sqlx::Error`。
3. **接口要窄**，supertrait 组合：`OrderReader` / `OrderWriter` / `OrderSearch` 分开，用例只声明所需。
4. **定义时就加 `Send + Sync`**（等到报错再补是大规模重构）。
5. **async 形态提前选**：

```rust
// 泛型注入、编译期已知实现：原生 AFIT（1.75+），零开销，但不兼容 dyn
pub trait OrderRepo: Send + Sync { async fn save(&self, o: &Order) -> Result<(), RepoError>; }
// 需要 Box<dyn>（运行时装配、fake 注入）：async-trait，一次 Box 分配 + 虚跳转
#[async_trait::async_trait]
pub trait OrderRepo: Send + Sync { async fn save(&self, o: &Order) -> Result<(), RepoError>; }
```

**AFIT 的隐藏限制**：`async fn` 无法在 trait 定义处给返回的 Future 加 `Send` bound（编译器会警告 `async_fn_in_trait`）——Future 是否 `Send` 取决于具体实现。用例要在 `tokio::spawn` 里跑时，需手写脱糖：`fn save(&self, o: &Order) -> impl Future<Output = Result<(), RepoError>> + Send;`，或直接用 `async-trait`（它默认加 `Send`）。这正是上表"需要 Send → async-trait"的深层原因。

决策树：需要 `Box<dyn>` 吗？否（泛型注入）→ 原生 AFIT；是且需 `Send` → `async-trait`（或 trait-variant 生成两版本）；是但不需 `Send` → 原生 AFIT + 手动 `Box<dyn Future>`（少见）。`async-trait` 默认加 `Send` bound，需要非 `Send` 内部状态时用 `#[async_trait(?Send)]`。

6. **泛型 vs dyn 裁决**：

| 维度 | 泛型 | `dyn` |
|---|---|---|
| 分发 / 内联 | 零开销 / 可内联 | 虚表 / 阻碍内联 |
| 编译时间 | 随"实现数 × 用例数"**组合膨胀** | 一份代码 |
| 签名传染 | bound 沿调用链上移 | 不传染 |

细粒度热路径 → 泛型；repo/gateway 大颗粒 → `dyn`；编译时间已痛 → `dyn`。**推荐混合**：用例内泛型保可测试性，组合根处 `Arc<dyn>` 做一次擦除，避免 `App<PgRepo, Stripe, SysClock, UuidGen>` 这种签名。"Rust 版 Java"批评的是 `dyn` + 共享可变 + 全局单例的**叠加**，不是 `dyn` 本身。

### 3.4 事务边界：Unit of Work + Outbox

领域层不知道事务，但"先扣库存、再写订单"必须原子。**事务边界归用例层**：应用层持有事务句柄，repo 方法接受事务而非连接池：

```rust
let mut tx = uow.begin().await?;           // Drop 未 commit 即自动回滚
tx.orders().save(&order).await?;
tx.outbox().append(&event).await?;         // 领域事件与业务数据同事务落 Outbox 表
tx.commit().await?;                        // 后台 worker 从 Outbox 投递到 MQ
```

Outbox 是"领域事件 + 事务"唯一可靠的组合方式——直接在事务里发 MQ，提交失败就有了幽灵事件。

### 3.5 领域保持同步，只有端口 async

领域逻辑用同步 `#[test]` 测，不需要 runtime，快一个数量级；I/O 全部收敛在端口 trait 之后。

### 3.6 组合根：`main` 必须薄（硬技术原因）

> `tests/` 集成测试只能链接 **lib target**，无法导入 `bin`。写在 `main.rs` 里的逻辑对集成测试**不可见**。

```rust
// lib.rs 承载 run()；main.rs 一行
pub async fn run(config: Config) -> anyhow::Result<()> { /* 装配 + 启动 + 优雅关停 */ }
```

组合根四个职责：读配置 → 建适配器 → 注入用例 → 启动并等待信号。Rust 不需要 DI 框架：构造函数 + 泛型/`dyn` 参数就是依赖注入，组合根是唯一知道所有具体类型的位置。**禁止 `once_cell` / `lazy_static` 全局单例**——那会让"谁依赖谁"从代码里消失。

### 3.7 什么时候不要分层 + 规模演进

CLI 小工具 / 纯算法库 / 原型 / 只有单一数据源 / 团队 <3 人 → 单 crate。**判据：分层价值 = 变更轴数量 × 变更概率。轴不变，分层就是纯负担。**

| 规模 | 结构 |
|---|---|
| <5k 行 | 单 crate，模块化，trait 只出现在最痛处 |
| 5k–50k | workspace，`domain` 独立成 crate 锁死方向 |
| >50k | 多适配器 crate、feature 门控、组合根显式装配、`cargo xtask` |

**模块化单体是合理默认**。拆服务的真实理由：独立伸缩 / 独立部署 / 故障隔离 / 团队自治。**不是**：代码库太大、微服务流行。

---

## 4. 领域建模：让非法状态不可表示

### 4.1 Newtype（Parse, Don't Validate）

三重职责：**语义区分**（`UserId` 传反给 `OrderId` 参数时编译报错）、**校验前移**、**突破孤儿规则**。

```rust
pub struct Email(String);
impl Email {
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        let norm = raw.trim().to_ascii_lowercase();   // 规范化只在唯一入口做
        if !norm.contains('@') { return Err(EmailError::Malformed); }
        Ok(Self(norm))
    }
    pub(crate) fn from_db(raw: String) -> Self { Self(raw) }  // 逃生舱，注明用途
    pub fn as_str(&self) -> &str { &self.0 }
}
```

**规范化的复利**：在唯一入口完成，`PartialEq` / `Hash` / `Ord` / `serde` **全部免费获得正确语义**；散落在使用处就会出现"同一邮箱两个 HashMap key"这类难查 bug。

**三个陷阱**：

- **`Borrow<str>` 与归一化冲突**：`Borrow` 要求 `Hash`/`Eq` 与 `str` 严格一致；做了 `to_lowercase` 就**绝不能**实现，否则 `HashMap<Email, V>` 用大写 `&str` 查找永远失败。极隐蔽的正确性 bug。
- **不实现 `Deref`**：那是智能指针语义，等于把约束又打开；只读访问用 `as_str()`。
- **金额禁 `f64`**：用 `Decimal` 或最小单位 `i64`，**币种必须跟金额走**（`Money { amount, currency }`）。

### 4.2 Enum 状态机：穷尽性 + 按值消费

```rust
// ❌ 三个 bool = 8 种组合，只有 4 种对应真实状态，另外 4 种非法（如"已发货但未付款"）
struct Order { paid: bool, shipped: bool, cancelled: bool }
// ✅ 4 种状态，全部合法；variant 可携带该状态专属数据
enum OrderState {
    Created  { created_at: DateTime<Utc> },
    Paid     { paid_at: DateTime<Utc>, amount: Money },
    Shipped  { shipped_at: DateTime<Utc>, tracking: TrackingNo },
    Cancelled{ cancelled_at: DateTime<Utc>, reason: CancelReason },
}
```

带数据的 variant 让"已支付订单必然有支付时间和金额"是**类型保证**，而非"字段可能为 null"的约定。**转换函数按值消费 `self`**（`fn ship(self)` 而非 `fn ship(&mut self)`）：所有权充当状态销毁器——旧状态在转换后物理不存在，不可能出现"已发货的订单又被改了 items"。`match` 穷尽 + 按值转换 = 重构安全网：新增状态时编译器点名每个未处理的 `match`。

### 4.3 Typestate 与三条崩塌边界

```rust
impl Order<Draft> { pub fn pay(self, r: Receipt) -> Result<Order<Paid>, PayError> { .. } }
impl Order<Paid>  { pub fn ship(self, t: TrackingNo) -> Order<Shipped> { .. } }
// Order<Draft> 上没有 ship()——编译期不可能"未支付先发货"
```

代价：类型参数污染签名、无法入 `Vec`、错误信息不友好、与 `&mut self` 风格冲突、状态多时爆炸。**三条崩塌线**（即 §0.2 三条边界在类型上的投影）：持久化崩塌（数据库行没有泛型参数，退化为 enum + 状态列）、运行时输入崩塌（反序列化回来的状态由外部决定）、并发参与崩塌（类型是单线程证据，跨任务不成立）。

**正解不是放弃，是分层**：typestate 负责"进行中的逻辑"，enum 负责"落盘的快照"，两者之间显式双向转换（重建走校验构造器 parse）。持久化层永远需要状态无关 DTO——不是麻烦，是边界的必然产物。**四条同时成立才用 typestate**：转换合法性编译期完全可知、状态数 ≤5、中间态不落盘、不跨任务。

### 4.4 RAII Guard：用 `Drop` 绑定配对操作

不要手动 lock/unlock、begin/commit。关键技巧——成功路径抑制 `Drop` 补偿：

```rust
impl TxGuard {
    pub fn commit(self) -> Result<(), TxError> {
        let this = std::mem::ManuallyDrop::new(self);  // 抑制 Drop 的回滚
        /* 真正提交 */
    }
}
impl Drop for TxGuard { fn drop(&mut self) { /* rollback */ } }
```

`ManuallyDrop` / `Option::take()` 在成功路径抑制 `Drop` 补偿，是让"要么提交要么回滚"成为类型保证的关键。适用于：锁、事务、指标计时、信号量许可、临时目录。

### 4.5 领域事件

用带数据的 `enum`（不用字符串标签），消费者穷尽 `match`，新增事件时编译器点名所有遗漏处理点。字段用过去式（不可变历史事实）。投递可靠性见 §3.4 Outbox。

---

## 5. 物理结构与依赖治理

**扁平优于嵌套**：`crates/*` 一层平铺（rust-analyzer 20 万行 32 crate 的实践；100 万行以内成立）。Cargo 命名空间本身是平的，树形目录制造第二套需人维护的层级。要对外发布的稳定 API 单独收进 `libs/`，且保证 `libs/` 不反向依赖 `crates/`。

```toml
[workspace]
resolver = "3"                 # edition 2024，dev-dependencies 的 feature 合流更精确
members = ["crates/*", "xtask"]
[workspace.dependencies]       # 统一版本，防同一 crate 多版本
[workspace.package]            # 内部 crate：version="0.0.0" + publish=false
[workspace.lints]              # 成员 crate 用 [lints] workspace = true 继承，防标准漂移
```

**拆 crate 判据**（至少一条）：强制边界 / 编译时间 / 发布边界 / 团队边界 / 可选依赖。**"文件太大"不是理由**（用模块拆）。自动化脚本统一写成 `cargo xtask`，不要散落 Makefile / shell——可移植、可补全、跨平台。

### 5.1 Feature unification 陷阱

Cargo 规则：**同一构建里每个 crate 只编译一份，feature 取依赖图上的并集**。后果：A 依赖 `serde` 不开 `derive`、兄弟 crate B 开了 → A 实际运行在带 `derive` 的版本上，**你以为关掉的被别人打开了，且没有任何警告**；更糟的是 workspace 内测试的合流结果 ≠ 发布后下游各自的合流结果——feature 组合的语义会随消费者变化。

**对策**：① feature 设计为**纯加性**（开 = 多能力，关 = 少能力，绝不改变行为语义，`--all-features` 必须能通过）；② **正交且显式**：A 隐含 B 就显式写 `A = ["B"]`，不靠默认链隐式传递；③ 需要**严格隔离**时用独立 crate（`my-lib-json` / `my-lib-xml`）而非 feature；④ CI 跑三个维度：`--no-default-features`、`cargo hack --each-feature`、`cargo hack --feature-powerset --depth 2`；⑤ 新项目默认 `resolver = "3"`；⑥ 警惕 creep：一个 feature 是否至少有一个真实用户在用？

### 5.2 Semver 与编译时间

`#[non_exhaustive]` 让加 variant/字段非破坏（代价见 §6.7）；sealed trait 换来自由添加方法，但**端口 trait 不该 sealed**（它的目的就是让人实现）。MSRV 显式声明并进 CI 矩阵，提升视为 minor。用 `cargo-semver-checks` 自动检测 PR 的 breaking change；deprecate → 保留一个 minor → 下个 major 删除。

编译时间：开发循环用 `cargo check`；`[profile.dev] debug=0` + 依赖 `[profile.dev.package."*"] opt-level=2`；链接器 `mold`/`lld`；减少过程宏；`sccache`；`cargo build --timings` 定位元凶。

---

## 6. API 设计：契约的编译期表达

### 6.1 四判据

- **Unsurprising**：对齐惯例——`iter`/`iter_mut`/`into_iter` 三件套；`to_` = 有开销、`as_` = 零开销借用、`into_` = 消耗所有权。反例：给零开销借用起名 `to_xxx`。
- **Flexible**：收最宽类型（`impl IntoIterator<Item=T>` 而非 `&Vec<T>`）；**绝不 `&T` 进来再 clone**；把顺带算出的中间结果也返回（`HashMap::insert` 返回旧值、`binary_search` 返回插入点）。
- **Obvious**：`self`/`&self`/`&mut self` 本身传达资源语义；返回 `Result` 时文档写明每个变体的触发条件。
- **Constrained**：`#[must_use]`、newtype 区分参数、typestate builder 让漏填字段编译失败。

### 6.2 所有权签名决策（最常用的一张表）

| 需要 | 写 | 别写 |
|---|---|---|
| 只读 | `&T` | `T` 然后丢弃 |
| 要长期存储 | `T` | `&T` 然后 `.clone()` |
| 任意来源字符串只读 | `impl AsRef<str>` | `&String` / `&str`（后者可但更窄） |
| 要拥有字符串 | `impl Into<String>` | `&str` 然后 `.to_string()` |
| 可能改可能不改 | `Cow<'_, str>` | 总是返回 `String` |
| 要遍历 | `impl IntoIterator<Item=T>` | `&Vec<T>` |

**最关键一条**：不要让 `&T` 参数在函数内部 `.clone()`——隐藏成本，且剥夺调用方"我不需要这份数据了，你拿走"的选择权。

### 6.3 泛型 / `dyn` / 枚举三选一

```
实现集合编译期封闭？ → 是 → 【优先枚举分发】无虚表、可 #[cfg] 裁剪、match 穷尽点名遗漏
                    → 否 → 需要运行时替换？ → 是 → dyn（注意对象安全）
                                          → 否 → 泛型
```

**枚举分发是被低估的王者**（其他语言没有对应优势）：`match` 穷尽性 = 新增实现时编译器点名所有遗漏；可 `#[cfg]` 裁剪；无 `Box` 分配、可内联。**能数清有几个实现就用枚举。** 对象安全约束：无泛型方法、无按值 `Self` 返回/参数、无关联常量；原生 `async fn` 目前不兼容 `dyn`。

### 6.4 必须由你实现的 trait（孤儿规则决定了用户无法替你实现）

| 层 | 内容 |
|---|---|
| 第一层（永远要有） | `Debug`、`Clone`、`PartialEq`/`Eq` |
| 第二层（按语义） | `Default` / `Hash` / `Ord` / `Copy` / `Display` / `FromStr` / `TryFrom` / serde |
| 第三层 | **能 `Send + Sync` 就一定要是**——否则用户被迫到处 `Arc<Mutex<_>>` |
| 第四层（手感） | 为 `&T` / `&mut T` / `Box<T>` 也实现 |
| 包装类型 | `Deref`（只给真正的智能指针）/ `AsRef` / `From` / `Borrow`（要求等价，见 §4.1） |

### 6.5 `impl Trait` 的四个真实限制

① **参数位置不能 turbofish**：`fn f(x: impl Display)` 不能写 `f::<String>(...)`，需显式指定类型时只能用泛型参数；② **返回位置不能命名**：`-> impl Iterator<Item=T>` 调用方无法存储具体类型，要存储就用 `Box<dyn>` 或具名类型；③ **返回位置改具名是破坏性变更**（或反过来），慎选；④ **`impl Trait` in type alias 仍未稳定**：复杂返回类型起名字只能 `Box<dyn Trait>` 或 nightly。**判据**：要隐藏实现 → `impl Trait`；调用方可能要存储/传递具体类型 → 具名类型（最好 newtype 包装）。

### 6.6 Builder 与 doctest

必填项 ≤2 且无交叉约束 → **typestate builder**（编译错误质量远超运行时 `Err(MissingField)`）；必填项多或有交叉约束 → 运行时 `build() -> Result`。文档必须写 **Panics / Errors / Safety / Examples** 四节；`# ` 隐藏行用于 `use`；**doctest 是唯一能保证文档不脱节的机制**（`cargo nextest` 不跑 doctest，CI 要单独 `cargo test --doc`）。

### 6.7 `#[non_exhaustive]` 的内外规则

对下游会 `match` 的公共类型加（错误 enum 几乎总要）；**对内部类型不加**——穷尽匹配是你自己的重构安全网，新增 variant 时编译器点名所有未处理的 `match`，加了就等于把这个安全网关掉，且下游失去字面量构造（须配套 builder）。**判据：这个类型会被下游 crate `match` 吗？**

---

## 7. 错误工程：按消费者设计

### 7.1 三消费者模型

"库 thiserror、应用 anyhow"只是现象，本质是消费者要什么：

| 消费者 | 需要 | 形态 |
|---|---|---|
| 代码 | 可 match 的稳定变体 | `thiserror` 错误 enum（+`#[non_exhaustive]`） |
| 日志/追踪 | 因果链 + 结构化字段 | `#[source]` 链 + `Display` |
| 人（用户/运维） | 可行动、不泄漏内部 | 错误码 + 白名单消息 |

**第三行最常被忽略**：用户看到的、日志记的、内部抛的是三种东西，必须在 API 边界显式映射一次，不要把 `Display` 透给用户。

### 7.2 `#[from]` 丢上下文的机制

`#[from]` 自动生成 `From<E>`，**转换发生在哪一层，路径信息就在那层丢掉**——`From` 的签名没有参数可以携带"从哪来"。同一 `sqlx::Error` 从读路径和写路径冒出来，`#[from]` 一转就只剩 `DatabaseError`，调用方无法区分。

**判据**：同一底层错误可能在 **≥2 个语义不同的路径**出现 → 禁用 `#[from]`；只有上下文无关处和全局出口（`main` 的 anyhow）才适合自动转换。

```rust
.map_err(|e| RepoError::Read { table: "orders", source: e })?   // ✅ 显式保留路径
```

另：两个 variant 不能同时 `#[from]` 同一类型（`From` 实现冲突），其余用 `#[source]` + 手写；40+ variant 的巨型 enum 要按模块拆分再用 `#[error(transparent)]` 组合。

### 7.3 错误分类学（比 variant 更重要）

| 类别 | 本质 | 响应管道 |
|---|---|---|
| **外部错误** | 预期内必然发生（网络/DB 抖动） | 重试 / 熔断 / 降级 |
| **用户错误** | 请求侧（输入非法） | 400 + 提示，**绝不重试** |
| **内部错误** | bug（不变量破坏） | 告警 + 修代码 |

三者必须在类型里显式区分，否则监控无法分离信号——"外部抖动"和"代码 bug"混成一个 5xx 率，既不能自动重试也不知道该不该告警。

### 7.4 幂等性：重试的真正前提

"可重试"是必要非充分条件，真判据是"**这个操作重试两次会发生什么**"。非幂等写（扣款）+ 自动重试 = 重复扣款。**三种正解**：**幂等键**（推荐：同 key 只执行一次，重复请求返回首次结果）、**先落意图再执行**（状态机推进）、**执行前状态前置校验**。这是错误处理成熟度真正的分水岭。

### 7.5 panic 政策

panic = 约定被违反（不变量破坏、逻辑 bug），**不是失败路径**。用户输入错误、网络超时、文件不存在走 `Result`。`unwrap`/`expect` 分级（写进 CI）：生产代码**禁 `unwrap`**，`expect` 必须带理由（`clippy::unwrap_used` / `expect_used`）；`tests/` `examples/` `benches/` `xtask` 允许。服务边界用 `catch_unwind` 兜底防单请求 panic 杀进程；**跨 FFI 时 unwind 是 UB**（须 `abort` 或 `C-unwind`）。panic 率 = bug 率的监控信号，混用可恢复错误会污染它。

### 7.6 日志只打一次

每一层都 `log::error!` 是最常见反模式——同一次失败产生五条日志，运维无法判断真实影响面。**规则：错误只传递，在请求边界（handler 顶层或 `main`）统一记录一次。**

---

## 8. 并发与异步：所有权决定策略

### 8.1 共享状态决策树

```
需要跨任务共享状态？
├─ 否 ──► 【最佳】单 owner + 消息传递（actor）
├─ 只读 ──► Arc<T>（T 不可变），【不需要锁】——最常见的过度加锁
├─ 写少读多 ──► Arc<RwLock<T>> 或 ArcSwap<T>（后者无锁读，更好）
├─ 写频繁/热路径 ──► 分片 Vec<Mutex<Shard>> 按 key 哈希（一把锁变 N 把）
├─ 只需计数 ──► AtomicU64（Ordering 要选对）
└─ 真要共享可变 ──► Arc<Mutex<T>>，最后手段（先过审计五问）
```

**被低估的是分片**：把一把大锁变 N 把小锁，冲突概率降 N 倍（`dashmap` 是成熟实现）。在"高频写入、key 空间大"的场景（缓存、会话存储），分片比消息传递更简单，比大锁快一个数量级。

**审计五问**（每次看到 `Arc<Mutex<T>>`）：① 真的需要共享可变吗？② 能消息传递吗？③ 能把状态限制在一个任务里吗？④ 临界区多粗？里面有 `.await` 吗？⑤ 能分片吗？`Arc<Mutex<T>>` 是 Rust 的"全局变量诱惑"，绝大多数过不了五问——这不是语言能力问题，是数据流没设计。

### 8.2 Tokio 三条铁律

1. **单段不 yield 控制在 ~100µs**——协作式 work-stealing，一个 worker 被长计算占住，同队列任务一起饥饿。
2. **阻塞放对池子**：同步阻塞 I/O（旧 DB 驱动、文件）→ `spawn_blocking`（默认上限 512 的阻塞池）；CPU 密集（加密/压缩/解析）→ `rayon` 或专用池。**别把 CPU 任务堆进 `spawn_blocking`**——它的队列默认很大，会被填满饿死真正的阻塞 I/O。
3. **`tokio::spawn` 集中在组合根/服务顶层**——散落的 spawn = 任务生命周期无人负责，是真实事故源头（见 §8.7）。

### 8.3 超时、重试、背压

- **所有外部调用必须有超时**，区分 connect / read / 总超时（只设总超时会掩盖"连上了但读不动"）。
- **重试要有边界**：指数退避 + 抖动 + 最大次数 + **只对幂等操作重试**（§7.4）。
- **背压用有界 channel**：`mpsc::channel(n)` 满时生产者 `.await`。**`unbounded_channel` 是内存泄漏的另一种写法**；限流用 `Semaphore` 或 `tower::limit`。

### 8.4 统一视角：部分进度

取消安全（§8.5）、幂等性（§7.4）、优雅关停（§8.7）看起来分散在三章，其实是**同一个结构**——任务被中断后留下部分状态：

| 场景 | 留下的"部分进度" |
|---|---|
| 取消安全 | Future 被 drop 时，操作读/写了一半 |
| 幂等性 | 重试时，第一次执行可能已经完成了一部分副作用 |
| 优雅关停 | shutdown 时，请求处理了一半、事务未提交 |

**统一判据**：设计任何长操作时问一句——**中断后重启，它能否安全继续？**

**统一解法**：把长操作建模为**可恢复的状态机**，让"进度"变成显式状态而非隐式残留：取消安全 → **阶段标记**（"读到第几阶段"入状态，重试从断点继续）；幂等性 → **先落意图**（intent 就是状态机的第一格）；关停 → **drain 步骤**（把"在途"显式化成一个阶段，可等待、可超时）。**不要让进度藏在栈里，要让它待在状态里。**

### 8.5 取消安全：异步最深的水

机制：`select!` 每次循环把未完成分支的 Future **drop**，**所有 `.await` 都是潜在取消点**。判断三问：Q1 操作分阶段吗？（读 header → 读 body 是两阶段）Q2 阶段之间重试幂等吗？（重读 header 会不会读成 body 数据）Q3 被 drop 时已产生的副作用能撤销吗？**三个都"否"才能裸放 `select!`**。

| 操作 | 安全 | 原因 |
|---|---|---|
| `mpsc::Receiver::recv()` / `sleep()` / `oneshot` | ✅ | 无副作用、无状态 |
| `AsyncReadExt::read` / `read_exact` / `read_line` | ❌ | 读一半被丢，**已读字节永久丢失**，重试导致流错位 |
| 非幂等写（扣款、发消息） | ❌ | 副作用重复执行 |
| 数据库事务内操作 | ❌ | drop 时行为依赖驱动实现 |
| 嵌套 `select!` | ❌ | 外层 drop 时内层状态丢失 |

**首要对策**：非取消安全操作**移出 `select!`**——select 里只等命令和 shutdown，收到后在外面完整执行：

```rust
// rx、shutdown 均需声明为 mut；read_exact 不在 select 内，不会被中途丢弃
let cmd = tokio::select! { Some(c) = rx.recv() => c, _ = shutdown.changed() => break };
let frame = stream.read_exact(&mut buf).await?;
```

备选：阶段标记状态机（断点续传）、`CancellationToken` + 任务内部检查（把取消变成协作式而非 drop 式）、选用文档明确标注 cancel-safe 的原语。`biased;` 只改变分支优先级，**不解决**取消不安全。

### 8.6 锁不跨 `.await`——与 `parking_lot` 陷阱

```rust
// ✅ 临界区只做同步快照，await 在锁外
let url = { state.lock().clone_url() };
let r = fetch(&url).await;
```

**关键认知**：编译器判 `Send` **不做控制流分析**——即使"逻辑上"没跨 await，只要 guard 在 await 点还活着（作用域未结束）就判非 `Send`。这不是编译器笨，是**保守正确**：它无法证明 `other().await` 不会访问 `g`。解法是显式作用域。

| 锁 | guard 是 `Send` | 跨 await 编译器会拦吗 |
|---|---|---|
| `std::sync::Mutex` | ❌ | ✅ **会拦** |
| `parking_lot::Mutex` | ✅ | ❌ **不拦——静默通过，然后死锁** |
| `tokio::sync::Mutex` | ✅ | ❌ 不拦（专为跨 await 设计） |

**用 `parking_lot` 的异步项目，"锁不跨 await"从编译期保证降级为纯纪律**——必须开 `clippy::await_holding_lock` 补位。

### 8.7 任务生命周期：`JoinHandle` 陷阱与关停四步

**`JoinHandle` 被 drop 不会 abort 任务**——只是放弃等待。事故形态：break 出主循环后，此前 `spawn` 的任务还在后台写数据库。正解：`JoinSet`（drop 时 abort 全部）+ `CancellationToken`。**结构化并发要义：子任务的生命周期不得超过父任务。**

```rust
let token = CancellationToken::new();
let mut set = JoinSet::new();
// 注意：mpsc 的 Receiver 不可 Clone（单消费者），多 worker 共享消费必须包一层：
let rx = Arc::new(tokio::sync::Mutex::new(rx));
for _ in 0..workers { set.spawn(worker_loop(rx.clone(), token.child_token())); }
tokio::signal::ctrl_c().await?;
token.cancel();       // 广播取消
set.shutdown().await; // 等待，drop 时 abort 剩余任务
```

> 这个 `Arc<Mutex<Receiver>>` 是**少数被接受的例外**——它是"多消费者分摊一个 mpsc 通道"的标准写法，不是 §8.1 批判的那种把共享可变当默认。另一个选项是 `async-channel`（接收端可 Clone，无锁多消费）。

**关停四步**：① 停止接受新请求 → ② 广播取消 → ③ 等待在途（**必须带超时兜底，否则一个卡住的任务让服务永远退不掉**）→ ④ 关连接并 flush 日志/指标。

---

## 9. 可观测性：为机器与人同时设计

用 `tracing` 不用 `log`：`#[instrument]` 自动生成 span，**span 可跨 `.await` 传递上下文**（`request_id` / `trace_id` 自动继承）——这是 async 时代 `log` 不够用的机制性原因。

```rust
// ❌ 动态 span 名：每个 user_id 生成一个新的 span 名 = 新时间序列
let span = tracing::info_span!("load_user_{user_id}");
// ✅ 静态 span 名 + 动态 field
let span = tracing::info_span!("load_user", user_id = %user_id);
```

**注意**：`#[instrument(name = "...")]` 的 `name` 只接受**字符串字面量，不支持 `{}` 插值**——想用动态名字只能走 `tracing::info_span!` 这类运行时构造 API。

**基数控制（事故高发区）**：指标系统（Prometheus / Datadog）存储成本与**时间序列数量**成正比，span 名是时间序列的维度之一——用 `user_id` 做 span 名会创建百万级时间序列，直接打爆指标后端。规则：span **名**必须静态低基数（`http_request` / `place_order` / `postgres.query`）；高基数数据（`user_id` / `order_id` / `email`）放 **field**；日志消息模板保持静态（便于聚合），动态值放 field（便于查询）；敏感字段用 `secrecy` 或自定义 `Debug` 脱敏。这条与直觉相反——直觉是"把 ID 放进名字更好查"，但实际是指标后端先崩。

- **静态消息 + 动态 field**：`error!(user_id = %id, "failed to load user")`。
- **指标至少四类**：QPS、延迟**直方图**（非平均值，长尾被平均值掩盖）、错误率、**饱和度**（队列深度、连接池占用——比错误率更早发现问题）。
- **健康检查分两个**：liveness 只查自身（查依赖会导致 DB 抖动时被反复重启、级联雪崩），readiness 才查依赖。
- 日志只在请求边界打一次（§7.6）。

---

## 10. 测试：对编译期承诺的运行期验证

**可测试性是架构的验收标准**：测不下去先改架构，不要先加 mock 框架。

| 测试困难 | 真实根因 | 解法 |
|---|---|---|
| 要启动数据库 | 业务依赖了具体仓储 | 抽端口 trait |
| 要 mock 十几个东西 | 用例依赖过宽 | 窄接口 + supertrait 组合 |
| 领域要 `#[tokio::test]` | 领域混入 async/I/O | 领域保持同步纯函数（§3.5） |
| 时间相关不稳定 | 内部调 `Utc::now()` | 时间作为入参（§0.3-D） |

**Fake 优先于 mock**：手写 `InMemoryRepo`（30 行）优于 `mockall`——fake 有真实行为（存进去能取出来），mock 只验证"被调用了"测不出逻辑错；fake 逼你把 trait 设计窄；mock 与实现细节绑死，重构时全炸。**`mockall` 只用于没有 trait 的第三方 SDK 或纯交互断言。**

**分层与守卫工具**（呼应 §0.2）：领域层纯单测零 mock；应用层对端口做 fake；适配层用真实 DB（testcontainers）测 SQL 与契约；E2E 只覆盖关键主流程。序列化边界 → 往返 `proptest` + `insta` 快照；运行时输入边界 → parse 测试 + `cargo-fuzz`（不可信输入）；调度器边界 → `loom`（自研并发原语）+ `tokio-console` + 取消/关停演练。

要点：修 bug 先写失败测试；用 `cargo-nextest`（进程级隔离，防静态状态污染；不跑 doctest）；**别追 100% 覆盖率**，追求"核心领域 80%+"，为覆盖率写无断言测试是负资产。

---

## 11. 性能：先测量，分配优先

先测量再优化——Rust 的直觉经常错：零成本抽象意味着"看起来慢"的已被优化掉，**真瓶颈常在分配**。工具：`criterion` / `divan`、`flamegraph` / `perf`、`tokio-console`（异步任务级）、`heaptrack` / `dhat`。

按收益排序：① **减分配**（`with_capacity`、复用 buffer、`Cow`、`SmallVec`、Arena）② 减拷贝（`clone` 上 CI 门禁）③ **内存布局**（字段降序减 padding、AoS→SoA、`Option<Box<T>>` niche 优化）④ 热路径避免 `dyn` ⑤ 迭代器优于手写循环（边界检查被消除）。

Profile：`lto = "thin"`、`codegen-units = 1`、`panic = "abort"`、`[profile.dev.package."*"] opt-level = 2`。CI 跑 benchmark 对比，劣化 >5% 报警（不硬失败，CI 机器波动大）。**公共 API 优先清晰，热点路径再优化**——为省一次 clone 把接口搞成生命周期地狱是典型过早优化；先 clone 让代码正确，有 profile 证据再收。

---

## 12. unsafe 与 FFI

**收容原则**：crate 根默认 `#![forbid(unsafe_code)]`，只在必要模块 `#![allow]`；unsafe 集中在最小模块、**对外只暴露 safe API**；每个 unsafe 块写 `// SAFETY:` 说明为何满足前提（**义务与证据，不是复述操作**）；不变量写进模块文档（不是散落注释）。`unsafe_op_in_unsafe_fn = "deny"` 让 `unsafe fn` 内部也需显式 `unsafe {}`，防止无意扩大范围。

**合法理由只有三种**：编译器无法证明的不变量、FFI/系统调用、**有 profile 证据**的性能优化——没 profile 的"优化式 unsafe"应一律拒绝。工具：`cargo miri test`（UB）、`-Zsanitizer=address/thread`、`loom`、`cargo geiger`（审计依赖树 unsafe 用量）。

**FFI**：不跨边界传 `String`/`Vec`（布局不稳定），用 `#[repr(C)]` POD；**谁分配谁释放，不能跨分配器**；边界用 `catch_unwind` 拦 panic 转错误码（unwind 跨 FFI 是 UB）；**优先 `bindgen`**（手写 extern 会漏、会错、会随上游失效）。

---

## 13. CI 门禁与关键数字

```bash
cargo fmt --check                                            # 最快失败在前
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features && cargo test --doc         # nextest 不跑 doctest
cargo deny check                                             # 四段见下
cargo audit                                                  # CVE / yanked
cargo +1.85 check                                            # MSRV
cargo hack check --each-feature                              # 逐 feature 单测编译（快，PR 必跑）
cargo hack check --feature-powerset --depth 2                # 组合互检（慢，nightly / 发布前）
cargo check --no-default-features                            # 关掉默认 feature 也要能编
./scripts/check-arch.sh                                      # 依赖方向（§3.1）
```

**Clippy 分级，不要一刀切**：默认组（correctness/suspicious/style/complexity/perf）deny；`pedantic` **先 warn 再逐条评估转 deny**（一次性 deny 会让 `#[allow]` 污染代码，比不开还糟）；`nursery` 挑几条；`restriction` **绝不整组开**，只挑 `unwrap_used` / `expect_used` / `dbg_macro`。

**`cargo-deny` 四段**：`advisories`（漏洞/yanked）、`bans`（禁 crate、重复依赖、**架构边界**）、`licenses`（白名单，GPL 通常排除）、`sources`（防供应链攻击，`unknown-git = "deny"`）。

**PR 门禁要快（<10 分钟）**，慢的（miri、fuzz、bench）放 merge 后——超过 10 分钟人就会开始绕过它。**依赖引入评审**：它解决什么？有更小的替代吗？维护活跃度？许可证？传递依赖树多大？unsafe 用量？

| 关键数字 | 建议值 |
|---|---|
| async 单段不 yield | ~100µs |
| 单文件行数上限 | ~800 行（超了拆模块，不是拆 crate） |
| 领域层单测覆盖率 | 80%+ |
| PR CI 时长 | <10 分钟 |
| 关停 drain 超时 | 30 秒（按业务调整） |
| channel 容量 | 有界，按峰值 QPS × 处理耗时估算 |
| 扁平 workspace 适用规模 | ~100 万行以内 |
| 性能回归阈值 | 5%（报警，不硬失败） |
| 测试要 mock 的端口数 | >5 个 = 用例依赖过宽，重构信号 |

**重构信号汇总**：改一处半个项目重编译 / `Arc<Mutex<T>>` 出现 5 次 / 加功能要改 10 个文件 / 编译超 5 分钟 / `Cargo.toml` 有 40 个依赖——任一命中，先怀疑结构。

---

## 14. 反模式速查（症状 → 根因 → 解法）

| 症状 | 根因 | 解法 |
|---|---|---|
| `Arc<Mutex<dyn Service>>` 满天飞 | 把 Rust 当 Java，用运行时锁关掉编译期所有权保证 | 重设计数据流：消息传递/分片/不可变共享 |
| Stringly typed（状态用 `"active"`、裸 `u64`） | 没用类型建模，传反不报错 | newtype + enum + Decimal |
| 业务逻辑写在 handler | 翻译层与领域混杂 | handler 只解析/调用/转响应 |
| `Clone` 满足借用检查器 | 数据流没想清，clone 是掩盖而非解决 | 重构数据流（拆结构体、`split_at_mut`） |
| `Deref` 模拟继承 | 误用智能指针语义，实现细节泄漏成公共 API | 组合 + 显式方法转发 |
| `panic!` 做可恢复错误 | 污染"panic 率 = bug 率"信号 | `Result` + 分类学（§7.3） |
| 领域层 import `sqlx` 或签名泄漏 `sqlx::Error` | 依赖方向破了 | 端口 trait + `Cargo.toml` 物理隔离 |
| 40 variant 的巨型 error enum | 错误没按模块拆 | 拆错误类型 + `#[error(transparent)]` |
| 内部调 `Utc::now()` / `rand::random()` | 副作用未隔离 | 作为参数注入（§0.3-D） |
| 状态用多个 bool | 状态组合爆炸，多数非法 | 合成 enum + by-value 转换 |
| 提前抽象，无第二实现 | 可复用性导向而非可替换性导向 | 写死，等第二实现出现再抽 |
| 为省 clone 把接口生命周期复杂化 | 接口复杂度是永久税，clone 成本是局部可测的 | 先 clone 跑通，profiler 指出瓶颈再优化 |
| async 里跑阻塞 / CPU 堆在 async | 不懂协作式调度 | `spawn_blocking` / `rayon` |
| 持锁跨 `.await`（用 `parking_lot` 还不报错） | guard `Send` 差异 | 显式作用域 + `await_holding_lock` |
| `select!` 放 `read_exact` / 非幂等写 | 不懂取消安全 | 移出 select / 阶段状态机 |
| `unbounded_channel` | 没考虑背压，把背压问题变成 OOM | 有界 channel + Semaphore |
| 非幂等写 + 自动重试 | 未过幂等性就重试 | 幂等键 / 先落意图 / 状态前置校验 |
| `spawn` 散落、丢弃 `JoinHandle` | 不知 drop 不 abort | `JoinSet` + `CancellationToken` |
| 日志拼字符串 / 高基数入 span 名 | 不懂结构化日志与基数控制 | 静态消息 + field |
| 每层都打 error 日志 | 不知日志只应打一次 | 请求边界统一记录 |
| liveness 检查查 DB | 语义混淆，级联雪崩 | 依赖检查放 readiness |
| 测试必须启动 DB / 全靠 mockall | 边界缺端口 | 抽 trait + 手写 fake |
| 一上来拆 8 个 crate / 树形嵌套目录 | 过度设计 | 单 crate 起步 + `crates/*` 扁平 |
| 库里 `#[deny(warnings)]` | 新 lint 会破坏下游构建 | 门禁写进 CI 参数，不写进源码 |
| 无 profile 就写 unsafe | 用安全换微小性能 | 拒绝，要证据 |
| 全局可变状态（`lazy_static`） | 隐式依赖图，测试不可并行不可组合 | 组合根显式注入 |

---

## 15. 落地

### 15.1 只做三件事（按杠杆排序）

1. **统一错误处理**：三消费者模型 + 分类学 + 禁 `unwrap` + 层间翻译——贯穿每个签名，改得越晚越贵。
2. **锁死依赖方向**：`domain` 零 I/O，用 `Cargo.toml` + CI 脚本物理强制——这决定了未来 18 个月重构成本的上限。
3. **强制门禁**：`fmt` + `clippy -D warnings` + `deny` + `audit` + 架构检查进 CI。**规范能自动执行的才叫规范，靠人自觉的最后都会烂。**

### 15.2 三步推行（周 / 月 / 季）

- **第一周：物理边界与工具链**——建扁平 workspace，配 `cargo-deny` 锁死依赖方向，CI 强制 `clippy -D warnings` + 架构守护脚本。
- **第一月：错误与类型**——`thiserror` + `anyhow` 分层，禁 `unwrap`，关键路径显式 `map_err`，消灭裸 `String`/`u64` 全换带校验 newtype。定型后反向约束所有模块边界。
- **第一季：并发与架构**——排查 `Arc<Mutex<T>>` 用 Channel/Actor/分片重构；`main.rs` 瘦身为组合根，逻辑下沉到 `domain`/`application`。

### 15.3 设计评审十问

1. 非法状态能用类型消除吗？ 2. 外部依赖（DB/HTTP/时钟/随机/ID）都在 trait 后吗？
3. 端口签名泄漏基础设施类型了吗？ 4. 能不启动外部服务就测试吗？
5. 错误给谁看？能 match？因果链完整吗？ 6. 有共享可变状态？能改消息传递/分片吗？
7. 有超时吗？取消安全吗？锁跨 await 了吗？ 8. 公共 API 最小？`non_exhaustive` 用对了吗？
9. 有 doctest？核心规则有测试？ 10. 新依赖的理由/许可证/unsafe 用量清楚吗？

---

## 收束

文档会过时，测试会遗漏，review 会疲劳，编译器不会。把每一个"必须"翻译成编译器无法忽视的事实——依赖方向写进 `Cargo.toml`，暴露面写进可见性，约束写进类型，并发安全写进所有权。

同时清醒：编译期的能力止于**序列化边界、运行时输入、调度器**三条边界。边界内交给编译器，边界外交给测试、可观测性与 CI——而且**每条边界都有专属守卫工具**（往返属性测试、边界 parse、loom/tokio-console）。两半合起来，才是一套完整的体系。

在 Rust 里，"代码能编译过，它就大概率是正确的、安全的、符合架构预期的"——这句话成立的条件，就是你把边界内的一切都交给了编译器。
