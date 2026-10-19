//! 7z 进度流解析(协议见 _docs/progress.md):
//! `-bso0 -bsp1 -bse1` 下 stdout 只剩 `\r` 分段的进度帧
//! `  45% 2 + Novel\book.txt`。两个实测坑:压缩收尾百分比回退(钳位单调);
//! WARNING/ERROR 走 stdout(非进度尾巴兜底报错)。

/// 一帧进度:百分比与条目名都可有可无(7z 只在变化时输出对应部分)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SevenZTick {
    pub percent: Option<u8>,
    pub entry: Option<String>,
}

/// 非进度尾巴(WARNING/ERROR 兜底)保留上限
const OTHER_TAIL_CAP: usize = 4096;

/// stdout 缓冲上限(1MB):真实帧 < 300 字节;防 `-bs*` 参数在异常 7z 构建上
/// 未生效时,无 `\r` 的流把 buf 撑大内存
const BUF_CAP: usize = 1024 * 1024;

/// 实例与单条命令绑定(max_percent 钳位与 other_tail 跨 feed 累积,不跨命令复用)。
pub(crate) struct SevenZProgressParser {
    buf: Vec<u8>,
    /// 已见最大百分比(压缩收尾会回退, 钳位保证进度单调不倒退)
    max_percent: Option<u8>,
    /// 最近 4KB 的非进度文本(WARNING/ERROR 走 stdout 时兜底报错用)
    other_tail: String,
}

impl SevenZProgressParser {
    pub(crate) fn new() -> Self {
        Self {
            buf: Vec::new(),
            max_percent: None,
            other_tail: String::new(),
        }
    }

    /// 喂一段 stdout;完整的 `\r` 段立即结算,不完整尾部留待下次
    pub(crate) fn feed(&mut self, chunk: &[u8], on_tick: &mut dyn FnMut(&SevenZTick)) {
        self.buf.extend_from_slice(chunk);
        // 防御:异常 7z 构建 `-bs*` 未生效时流可全程无 `\r`,超 BUF_CAP 直接整段结算,
        // 文本归入 other_tail(自带 4KB 上限),避免 buf 无限增长
        if self.buf.len() > BUF_CAP && !self.buf.contains(&b'\r') {
            let rest = std::mem::take(&mut self.buf);
            self.take_segment(&rest, on_tick);
        }
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\r') {
            let seg: Vec<u8> = self.buf.drain(..=pos).collect();
            self.take_segment(&seg[..seg.len() - 1], on_tick);
        }
    }

    /// 流结束:结算最后一段(可能不带 `\r`),返回非进度尾巴供失败兜底
    pub(crate) fn finish(&mut self, on_tick: &mut dyn FnMut(&SevenZTick)) -> String {
        let rest = std::mem::take(&mut self.buf);
        self.take_segment(&rest, on_tick);
        std::mem::take(&mut self.other_tail)
    }

    fn take_segment(&mut self, seg: &[u8], on_tick: &mut dyn FnMut(&SevenZTick)) {
        let text = String::from_utf8_lossy(seg).into_owned();
        match parse_fields(&text) {
            Some(mut tick) => {
                // 钳位单调: -tzip 压缩收尾实测会回退(98%→37%)
                tick.percent = tick.percent.map(|p| {
                    let m = self.max_percent.unwrap_or(0).max(p);
                    self.max_percent = Some(m);
                    m
                });
                on_tick(&tick);
            }
            None => {
                if !text.trim().is_empty() {
                    self.other_tail.push_str(&text);
                    self.other_tail.push('\n');
                    let len = self.other_tail.len();
                    if len > OTHER_TAIL_CAP {
                        let cut = len - OTHER_TAIL_CAP;
                        let boundary = (cut..=len)
                            .find(|i| self.other_tail.is_char_boundary(*i))
                            .unwrap_or(len);
                        self.other_tail.drain(..boundary);
                    }
                }
            }
        }
    }
}

/// 解析一帧:`  45% 2 + Novel/book.txt` → (45, "Novel/book.txt");不是进度帧返回 None
fn parse_fields(seg: &str) -> Option<SevenZTick> {
    let seg = seg.trim_start();
    let pct_end = seg.find('%')?;
    let digits = seg[..pct_end].trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let percent = digits.parse::<u8>().ok().filter(|p| *p <= 100)?;
    // 剩余形如 "2 + Novel/book.txt" / "2 -" / "":取 +/- 标记后的名字
    let rest = seg[pct_end + 1..].trim();
    let entry = rest.split_once(['+', '-']).and_then(|(_, name)| {
        let name = name.trim();
        (!name.is_empty()).then(|| name.replace('\\', "/"))
    });
    Some(SevenZTick {
        percent: Some(percent),
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(p: &mut SevenZProgressParser, text: impl AsRef<[u8]>) -> Vec<SevenZTick> {
        let mut ticks = Vec::new();
        p.feed(text.as_ref(), &mut |t| ticks.push(t.clone()));
        ticks
    }

    #[test]
    fn parses_percent_and_entry_from_carriage_return_stream() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(
            &mut p,
            "  45% 2 + Novel\\book.txt\r  46% 2 + Novel\\book.txt\r",
        );
        assert_eq!(
            ticks,
            [
                SevenZTick {
                    percent: Some(45),
                    entry: Some("Novel/book.txt".into())
                },
                SevenZTick {
                    percent: Some(46),
                    entry: Some("Novel/book.txt".into())
                },
            ]
        );
    }

    #[test]
    fn keeps_incomplete_tail_until_complete() {
        let mut p = SevenZProgressParser::new();
        assert!(feed_all(&mut p, "  4").is_empty());
        let ticks = feed_all(&mut p, "5% 1 + a.txt\r");
        assert_eq!(
            ticks,
            [SevenZTick {
                percent: Some(45),
                entry: Some("a.txt".into())
            }]
        );
    }

    #[test]
    fn clamps_percent_regression_monotonically() {
        // 实测: -tzip 压缩收尾 98% → 37%, 钳位后不倒退
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  98% 1 + a.bin\r  37% 1 + a.bin\r");
        assert_eq!(
            ticks,
            [
                SevenZTick {
                    percent: Some(98),
                    entry: Some("a.bin".into())
                },
                SevenZTick {
                    percent: Some(98),
                    entry: Some("a.bin".into())
                },
            ]
        );
    }

    #[test]
    fn percent_without_entry_yields_none_entry() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  45%\r");
        assert_eq!(
            ticks,
            [SevenZTick {
                percent: Some(45),
                entry: None
            }]
        );
    }

    #[test]
    fn minus_marker_and_trailing_spaces_parse() {
        // 解压帧: "  30% 5 - Novel/book.txt"(名字后的空格丢弃)
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  30% 5 - Novel/book.txt  \r");
        assert_eq!(
            ticks,
            [SevenZTick {
                percent: Some(30),
                entry: Some("Novel/book.txt".into())
            }]
        );
    }

    #[test]
    fn banner_and_error_text_collected_in_other_tail() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "0M Scan\r  45% 1 + a.txt\r");
        feed_all(&mut p, "WARNING: no such file\r");
        let tail = p.finish(&mut |_| {});
        assert!(tail.contains("0M Scan"), "{tail}");
        assert!(tail.contains("WARNING: no such file"), "{tail}");
    }

    #[test]
    fn non_utf8_bytes_become_lossy_without_panic() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, b"  45% 1 + \xff\xfe.bin\r");
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].percent, Some(45));
        assert!(ticks[0].entry.as_deref().unwrap_or("").contains('\u{FFFD}'));
    }

    #[test]
    fn percent_over_100_is_treated_as_other_text() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "  145% 1 + x\r");
        let tail = p.finish(&mut |_| {});
        assert!(tail.contains("145%"), "{tail}");
    }

    #[test]
    fn empty_segments_are_ignored() {
        let mut p = SevenZProgressParser::new();
        assert!(feed_all(&mut p, "\r\r\r").is_empty());
        assert!(p.finish(&mut |_| {}).trim().is_empty());
    }

    #[test]
    fn finish_flushes_remaining_segment_without_cr() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "  45% 1 + a.txt");
        let mut ticks = Vec::new();
        p.finish(&mut |t| ticks.push(t.clone()));
        assert_eq!(
            ticks,
            [SevenZTick {
                percent: Some(45),
                entry: Some("a.txt".into())
            }]
        );
    }

    #[test]
    fn percent_and_plus_inside_entry_name_do_not_confuse_parsing() {
        // 名字里含 % 与 +/-:锁定 split_once 首次出现语义(第一个标记后的整段都是名字)
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  45% 2 + 50%off-final.txt\r");
        assert_eq!(
            ticks,
            [SevenZTick {
                percent: Some(45),
                entry: Some("50%off-final.txt".into())
            }]
        );
    }

    #[test]
    fn other_tail_truncates_multibyte_text_on_char_boundary() {
        // 超过 4KB 的多字节非进度文本:截断必须切在字符边界上(不 panic)且保留最近内容
        let mut p = SevenZProgressParser::new();
        // 3000 × "中" = 9000 字节,一次 feed 且不带 `\r`,由 finish 结算
        let chunk = "中".repeat(3000);
        p.feed(chunk.as_bytes(), &mut |_| {});
        let tail = p.finish(&mut |_| {});
        assert!(!tail.is_empty());
        assert!(tail.len() <= OTHER_TAIL_CAP, "tail len = {}", tail.len());
        assert!(
            tail.chars().all(|c| c == '中' || c == '\n'),
            "截断切碎了字符: {tail:?}"
        );
    }
}
