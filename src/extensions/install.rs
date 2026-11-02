//! 插件 ZIP 的后台下载与安装。
//!
//! 与解码器 `decoders::install` 同形：WinHTTP 在后台线程跑，UI 250 ms 轮询进度；
//! 落盘前**不校验清单指纹**（2026-11-02 用户口径），解压、读 manifest 核 `id`、再整体换目录。
//! 这里只碰文件系统，不写配置、不碰信号。
use std::sync::Mutex;

use crate::core::config::SourceId;

use super::AvailablePlugin;

const MAX_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub id: String,
    pub got: u64,
    pub total: u64,
}

impl Progress {
    pub fn percent(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        ((self.got.min(self.total) * 100) / self.total) as u8
    }
}

struct Running {
    id: String,
    got: u64,
    total: u64,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    done: Option<Outcome>,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    running: None,
    done: None,
});

fn report(id: &str, got: u64) {
    let mut g = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = &mut g.running {
        if r.id == id {
            r.got = got.min(r.total);
        }
    }
}

pub fn progress() -> Option<Progress> {
    let g = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    g.running.as_ref().map(|r| Progress {
        id: r.id.clone(),
        got: r.got,
        total: r.total,
    })
}

pub fn start(e: &AvailablePlugin, src: SourceId) -> Result<(), String> {
    // 协议 / 宿主版本不满足：装了也跑不起来（清单允许列出，但入口必须拒）。
    if !e.compatible() {
        return Err("这一枚与当前宿主不兼容".to_string());
    }
    let url = e.download_url(src).map(str::to_string);
    let local = super::local_package_path(e).filter(|p| p.is_file());
    if url.is_none() && local.is_none() {
        return Err("这一枚在当前来源还没有发布地址".to_string());
    }
    {
        let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
        if let Some(r) = &g.running {
            return Err(format!("正在下载「{}」，等它完成", r.id));
        }
        g.running = Some(Running {
            id: e.id.clone(),
            got: 0,
            total: e.size,
        });
    }
    let job = e.clone();
    std::thread::spawn(move || {
        let result = (|| {
            let bytes = match &url {
                Some(url) => {
                    match crate::platform::http::get_with_progress(url, MAX_BYTES, |got| {
                        report(&job.id, got)
                    }) {
                        Ok(bytes) => bytes,
                        // 本地已有同版本包时允许离线安装；远程 404 不再把用户卡死。
                        Err(_) => match &local {
                            Some(path) => {
                                std::fs::read(path).map_err(|e| format!("读本地插件包失败：{e}"))?
                            }
                            None => return Err("联网下载失败".to_string()),
                        },
                    }
                }
                None => match &local {
                    Some(path) => {
                        std::fs::read(path).map_err(|e| format!("读本地插件包失败：{e}"))?
                    }
                    None => return Err("没有可用的下载地址".to_string()),
                },
            };
            super::install_bytes(&bytes, &job, true)
        })();
        let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
        g.running = None;
        g.done = Some(Outcome {
            id: job.id,
            name: job.name,
            result,
        });
    });
    Ok(())
}

pub fn poll() -> Option<Outcome> {
    let mut g = SLOT.lock().unwrap_or_else(|x| x.into_inner());
    g.done.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_clamps_and_handles_zero_total() {
        let p = Progress {
            id: "a".into(),
            got: 150,
            total: 100,
        };
        assert_eq!(p.percent(), 100);
        let z = Progress {
            id: "a".into(),
            got: 0,
            total: 0,
        };
        assert_eq!(z.percent(), 0);
    }
}
