//! 任务取消旗标:把"UI 点击取消"桥接成 core 进度回调的 `ControlFlow::Break`。

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zip_tool_core::Progress;

#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// 包装进度通知为 core 回调:先转发进度,再检查取消。
    /// 取消后返回 Break,core 会在当前条目完成后停止。
    /// 按值获取(CancelFlag 是 Arc 克隆体,克隆后传入即可)。
    pub fn into_callback<F: Fn(&Progress)>(
        self,
        notify: F,
    ) -> impl Fn(&Progress) -> ControlFlow<()> + use<F> {
        move |p| {
            notify(p);
            if self.is_cancelled() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn forwards_progress_until_cancelled() {
        let flag = CancelFlag::new();
        let seen = RefCell::new(Vec::new());
        let cb = flag.clone().into_callback(|p| seen.borrow_mut().push(p.done));

        assert!(matches!(cb(&Progress { done: 1, total: 3, entry: "A".into() }), ControlFlow::Continue(())));
        assert!(matches!(cb(&Progress { done: 2, total: 3, entry: "B".into() }), ControlFlow::Continue(())));

        flag.cancel();

        assert!(matches!(cb(&Progress { done: 3, total: 3, entry: "C".into() }), ControlFlow::Break(())));
        assert_eq!(*seen.borrow(), [1, 2, 3], "取消前的每次进度都应转发");
    }
}
