//! 响应式动态列表 widget（`Element::list_signal` 的内部驱动）。
//!
//! `DynList<T>` 挂载在滚动容器节点上，当绑定的 `Signal<Vec<T>>` 版本号变化时，
//! 在 `on_update`（layout 前回调）中清空旧子节点、按新数据重建新子节点。
//!
//! 外部只通过 `Element::list_signal` 使用，无需直接构造本类型。

use crate::core::{EventCtx, Widget};
use crate::event::Event;
use crate::geometry::{Rect, Size};
use crate::render::Canvas;
use crate::signal::{Signal, SignalScope};
use crate::style::Style;
use crate::text::TextEngine;

pub struct DynList<T: Clone + 'static> {
    data: Signal<Vec<T>>,
    row_fn: Box<dyn Fn(T) -> super::Element>,
    last_version: u64,
    /// 当前这批行在构建期创建的信号。下轮重建先整批回收，避免
    /// "`row_fn` 里 `signal(..)` → 每次数据变化多漏一批槽位"。
    rows: SignalScope,
}

impl<T: Clone + 'static> DynList<T> {
    pub fn new(data: Signal<Vec<T>>, row_fn: impl Fn(T) -> super::Element + 'static) -> Self {
        Self::with_scope(data, row_fn, SignalScope::new())
    }

    /// 接管首批行的信号作用域：`Element::list_signal` 在构建初始行时已开了一个作用域，
    /// 把它交给 widget，首批行才能在第一次重建时被一并回收（否则永久漏一代）。
    pub(super) fn with_scope(
        data: Signal<Vec<T>>,
        row_fn: impl Fn(T) -> super::Element + 'static,
        rows: SignalScope,
    ) -> Self {
        Self {
            last_version: data.version(),
            data,
            row_fn: Box::new(row_fn),
            rows,
        }
    }
}

impl<T: Clone + 'static> Widget for DynList<T> {
    fn on_update(&mut self, ctx: &mut EventCtx) {
        let ver = self.data.version();
        if ver == self.last_version {
            return;
        }
        self.last_version = ver;

        let self_id = ctx.id();
        let items = self.data.get();
        // 先拆字段：`rows.collect` 要 `&mut self.rows`，闭包里还要读 `self.row_fn`。
        let Self { row_fn, rows, .. } = self;
        let tree = ctx.tree_mut();

        // 移除当前所有子节点（递归释放子树 arena slot）
        let old_children: Vec<_> = tree
            .get(self_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child in old_children {
            tree.remove(child);
        }
        if let Some(n) = tree.get_mut(self_id) {
            n.children.clear();
        }
        // 旧行的节点已经没了，其构建期信号同刻回收——两者生命周期必须一致，否则
        // 要么漏槽位、要么让还活着的行读到已回收的信号。
        rows.dispose();

        // 按新数据重建子节点，新一批构建期信号归 `rows` 所有。
        rows.collect(|| {
            for item in items {
                let el = row_fn(item);
                let child_id = el.build(tree);
                tree.add_child(self_id, child_id);
            }
        });
    }

    // DynList 自身无视觉内容；背景/边框由容器 Style 处理。
    fn measure(&self, _avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        Size::ZERO
    }
    fn paint(
        &self,
        _bounds: Rect,
        _content: Rect,
        _focused: bool,
        _enabled: bool,
        _canvas: &mut dyn Canvas,
        _style: &Style,
    ) {
    }
    fn on_event(&mut self, _ctx: &mut EventCtx, _ev: &Event) -> bool {
        false
    }
}

/// ★ RuPix 补丁 08 —— `list_signal` 的滚动代理：让 `DynList` 与 `ScrollWidget` 共存。
///
/// # 踩的是什么坑
/// `Element::list_signal` 的容器是 `Self::scroll().fill()`，而 `scroll()` 的本职就是
/// 装上 [`super::containers::ScrollWidget`]（滚轮滚动 + 滚动条拖动）。紧接着构造器又调
/// `set_widget(Box::new(DynList{..}))` 挂上本文件的主角——而 `set_widget` 是**覆盖**
/// 语义（`self.widget = w`），`ScrollWidget` 就被静默丢掉了（框架自己在 `Element::widget`
/// 的断言里警告过：`在该节点已有控件，再挂会把它静默替换掉`；`set_widget` 作为内部入口
/// 恰好绕过了那道守卫）。
///
/// `DynList::on_event` 恒 `false`，替换之后**再没有任何代码处理滚轮与滚动条拖动**。
/// 症状极具迷惑性：
/// - `Layout::Scroll` 仍在 → `measure_scroll`/`arrange_scroll` 照常跑，`content_h`、
///   `view_h`、`max_scroll` 全都算对，滚动条也**画得出来**；
/// - 但事件侧无人接手 → `scroll_y` 永远是 0。
/// 即"滚动条看得见、拖不动、滚轮也不滚"。
///
/// # 为什么不能在 DynList 里直接转发
/// `DynList` 同样被 `host_signal`（普通 `col` 容器）复用，那个场景没有可滚容器，
/// 硬塞一个 `ScrollWidget` 会让不可滚的列容器吞掉滚轮事件、外层的滚动反而失灵。
///
/// # 做法
/// 组合而非替换：`on_update`/`measure`/`paint` 走 `DynList`，`on_event` 委托给
/// `ScrollWidget`。两者各司其职，共用一个 widget 槽位。
pub struct DynListWithScroll<T: Clone + 'static> {
    list: DynList<T>,
    scroll: super::containers::ScrollWidget,
}

impl<T: Clone + 'static> DynListWithScroll<T> {
    pub fn new(list: DynList<T>) -> Self {
        Self {
            list,
            scroll: super::containers::ScrollWidget::default(),
        }
    }
}

impl<T: Clone + 'static> Widget for DynListWithScroll<T> {
    fn on_update(&mut self, ctx: &mut EventCtx) {
        // 数据版本变了就重建行——与纯 DynList 完全一致。
        self.list.on_update(ctx);
    }

    fn measure(&self, avail: Size, style: &Style, text: &mut dyn TextEngine) -> Size {
        self.list.measure(avail, style, text)
    }

    fn paint(
        &self,
        bounds: Rect,
        content: Rect,
        focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        self.list
            .paint(bounds, content, focused, enabled, canvas, style);
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        // 滚轮 / 滚动条拖动一律交给真正管滚动的 widget；DynList 不处理任何事件。
        self.scroll.on_event(ctx, ev)
    }
}
