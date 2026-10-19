# 进度流协议(7z -bso0 -bsp1 -bse1)

## 实测协议(NanaZip 25.01 / 7-Zip 23.01,2026-10)

- `7z a|x` 加 `-bso0 -bsp1 -bse1` 后 stdout 只剩进度段,形如
  `  45% 2 + Novel\book.txt`(`+`=添加 / `-`=解出;名字可含空格、可缺省),约 100ms 一帧。
- **帧分隔符随构建而异**(2026-10 CI 实测):
  - Windows 7-Zip(含 NanaZip):`\r` 分隔,整流可无 `\n`;
  - Unix p7zip(23.01)/ 7zz:用退格 `\b` + 空格擦除重绘,整流无 `\r`
    (64MB 随机数据实测:`\r`=0、`\b`=796)。
  解析器把 `\r` 与 `\b` 都视作帧边界;擦除产生的空/纯空格段会被忽略。
- 百分比在 `-tzip` 压缩收尾会回退(98%→37%):解析器钳位单调。
- 7z 解压 zip 的百分比随版本波动(NanaZip 对 .zip 只报 0%、96%):解压不依赖百分比,
  用"预扫大小(l -ba -slt 的 Size =)+ stdout 条目名首见累计"。
- WARNING/ERROR 即使 -bse1 也可能走 stdout:解析器收集最近 4KB 非进度文本,
  失败且 stderr 为空时兜底作错误输出(镜像 run_impl 的 stdout fallback 语义)。

## 线程模型

读者线程解析 stdout 并把 tick 发进 mpsc;主轮询线程(100ms POLL_INTERVAL)drain
channel 后在本线程回调。公共 API 的 `&dyn Fn` 回调因此不需要 `Sync`。
取消仍走 `CancelFlag` 轮询 kill(tick 不响应回调 Break)。

## 映射

- 压缩(7z 系):百分比 × 扫描期条目大小 → `Progress.done_bytes` 字节加权(见 compress.md §7)。
- 解压(7z 系):`l -ba -slt` 预扫每条目大小为权重,stdout 条目名首见 → 累加并上报。
- zip/unzip(Info-ZIP):无流式进度,维持条目/组级现状。
