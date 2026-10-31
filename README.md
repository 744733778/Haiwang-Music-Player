<p align="center">
  <img src="icon.png" width="160" alt="海王音乐播放器">
</p>

<h1 align="center">海王音乐播放器</h1>

<p align="center">libmpv + egui 写的 Windows 桌面音乐播放器 · 纯本地 · 约 12 MB · 解压即用</p>

---

从 [BYD-AplayerFoliaMix](https://github.com/744733778/BYD-AplayerFoliaMix) 提炼的最小桌面原型。只保留最必要的能力，重点做好**歌词**：多源在线匹配、取匹配度最高的歌词，保存到音频同目录。

- **成品下载**：见 [Releases](../../releases) —— `海王音乐播放器-win64-v4.zip`（内含 `rust-music.exe` + `libmpv-2.dll` + `icon.png` + 使用说明）
- **项目图标**：`icon.png`
- **支持系统**：Windows 7 / 10 / 11（64 位）

## 已实现

### 播放与媒体库
- **手动导入文件夹**：输入/选择目录，递归扫描 `mp3 / flac / m4a / aac / ogg / opus / wav / ape / dsf ...`，用 `lofty` 读取标题/歌手/专辑/时长（失败时回退文件名）
- **启动自动重扫**：第二次打开自动重新扫描上次导入的文件夹（可在设置里关掉），不用再点一次导入
- **libmpv 播放**：`libloading` 运行时动态加载，Windows 只需同目录放 `libmpv-2.dll`，Linux 用 `libmpv.so.2`
- **播放控制**：播放/暂停、上一首/下一首、进度条拖动、音量
- **打乱歌单**：点「🔀 打乱歌单」把左侧列表**真正重排**（不是随机抽歌），所以下一首永远是列表里的下一行，看得见、猜得到；当前播放的那首会被保留并继续播，点「↩ 原顺序」可复原。思路与原项目 `ExoPlayback.shuffleQueue()` 一致——不开 ExoPlayer/mpv 的 shuffleModeEnabled，因为那会让每次切歌重新随机，队列就不可信了

### 歌词
- **本地歌词解析**：同名 `.lrc` / `.txt` 或内嵌歌词；支持
  - 标准 `[mm:ss.xx]`
  - 增强逐字 `<mm:ss.xx>词` / `<起始|结束>词`（标签在词后）
  - KuGou `<mm:ss.xx|mm:ss.xx>词`（带词结束时间）
  - 逐字 `词[mm:ss.xx]`
  - `[offset:±ms]`、一行多时间戳、翻译合并
- **在线歌词下载**：4 个源合并检索，移植自 folia-major / folia 的匹配算法
  - **AMLL TTML 逐字库**（人工校对，质量最高，按 `api.amll.dev`）
  - **QQ 音乐**（QRC 逐字，自定义 3DES 解密 + zlib）
  - **网易云**（逐行 LRC + 翻译 `tlyric`）
  - **酷狗**（KRC 逐字，16 字节 XOR + zlib）
  - 顺序可在设置里用 ↑/↓ 调整，先命中高置信度的源就直接用，否则取全部源里的最高分
- **桌面歌词窗**：置顶、透明、无边框；显示当前行（逐字/逐行高亮）+ 翻译 + 下一行
- **等响度（ReplayGain）**：见下节

### 桌面歌词窗（Windows：自绘分层窗口）
egui 的 `show_viewport_immediate + with_transparent(true)` 在 Windows 上**不可靠**：glutin 给 WGL 只传 `WGL_TRANSPARENT_ARB`，真正的 per-pixel alpha 要靠像素格式的 `cAlphaBits=8` + DWM 按像素合成，远程桌面 / 云端虚拟显卡上经常整窗黑底。所以 Windows 版改成自己开一个 **Win32 分层窗口**：

- 用 `UpdateLayeredWindow` + 32 位 DIB：文字先用 GDI 画成白色遮罩（`ANTIALIASED_QUALITY` 灰度抗锯齿），再把亮度当 alpha、按行上色、逐像素预乘，交给 DWM 合成 → **透明是确定的**，与 GL 驱动无关
- 拖动：`WM_LBUTTONDOWN` → `ReleaseCapture()` + `SendMessage(WM_NCLBUTTONDOWN, HTCAPTION)`，直接进系统的模态拖动循环，窗口由系统移动，零延迟、不抖
- 逐字高亮由窗口线程按 33ms 节拍对「当前位置 + 已过时间」插值，所以 30fps 平滑，不受主界面刷新率影响
- 锁定 → `WS_EX_TRANSPARENT`（鼠标穿透）；背景即使设成 0% 也会留 1/255 的 alpha，否则整块区域在分层窗口上点不到、拖不动
- 外观设置：字号 16–72、已唱/未唱/翻译三色取色器、背景不透明度 0–100%、窗口宽高
- 其它平台（Linux）仍走 egui 子 viewport + 透明主窗口的老路子（`draw_lyric_viewport`）

## 等响度（ReplayGain）

移植自 BYD-AplayerFoliaMix 的 `LoudnessAnalyzer.kt` / `ReplayGainController.kt` / `ReplayGainAudioProcessor.kt`：

1. **测**：用一个独立的 mpv 实例把文件解码成 48kHz 单声道 WAV（格式覆盖面和播放器完全一致，ape/dsf/wv 都在内）→ 单声道化 → K 加权（38Hz 高通 Q0.5 + 1.5kHz +4dB 高架 Q0.707）→ 400ms 窗口 RMS → 取 95 分位 → `gain = 目标(-18dBFS) - 20log10(rms)`，夹在 ±20dB；同时记峰值
2. **存**：`loudness.json` 按「路径 + 文件大小 + 修改时间」缓存，文件变了自动重测；扫描边测边存，可以中断续跑
3. **用**：文件里的 ReplayGain 标签优先 → 本地测量（可自动/手动）→ 缺省增益；加 preamp；峰值保护时 `gain = min(gain, -20log10(peak))`；通过 mpv 的 `af` 链（`volume` 滤镜）生效

> 移植时修了原实现的一个 bug：RBJ 高架的 `a1` 应为 `2*((A-1)-(A+1)cos)`，Kotlin 版写成了负号，等于把滤波器接反了。修好后 A=1（0dB）时 b≡a，退化为直通，`shelf_is_flat_at_0db` 这个单测就是在钉这个行为。
>
> 另外两处 mpv 的坑：`af clr` 在 0.37 上返回 `MPV_ERROR_OPTION_FORMAT(-4)`，要用 `af set ""`；增益值不能写成 `-4.5dB`（前导 `-` 会被当成下一个选项），改用线性倍数。

## 运行依赖

| 平台 | 需要 |
| --- | --- |
| Windows 7 / 10 / 11 (64 位) | `libmpv-2.dll` 放在 `rust-music.exe` 同目录 |
| Linux | `libmpv2`（提供 `libmpv.so.2`） |

`libmpv-2.dll` 可从 [mpv 官方 Windows 构建的 dev 包](https://sourceforge.net/projects/mpv-player-windows/files/libmpv/)（`mpv-dev-x86_64-*.7z`）或 MSYS2 的 `mingw-w64-x86_64-mpv` 中提取。

不过官方那种包是「什么都有」的全量构建，解压出来的 `libmpv-2.dll` **122 MB**（`.text` 就 97 MB），对一个只放歌的程序太重。仓库发布包里带的是自编译精简版，**5.7 MB**，见下节。

## 精简版 libmpv 是怎么编出来的

只保留音频链路，把 GPU / 网络 / 字幕之外的重型依赖全部关掉。

依赖顺序（全部交叉编译成静态库，最后一起链进 DLL）：

| 组件 | 版本 | 关键开关 |
| --- | --- | --- |
| ffmpeg | 7.1 | `--disable-everything` + 显式白名单：mp3/mp3float/flac/vorbis/opus/aac/alac/ape/wavpack/dsd_*/pcm_*，封装 mp3/flac/ogg/wav/aiff/caf/mov/ape/dsf/wav/matroska/asf/mpc/tta，滤镜 volume/alimiter/aresample/aformat/format/anull/pan/amerge |
| freetype | 2.13.3 | 静态，不要 harfbuzz |
| fribidi | 1.0.16 | 静态 |
| libass | **0.14.0** | 静态；`--disable-fontconfig --disable-asm` |

> libass 0.15 起把 harfbuzz 变成**硬依赖**（0.15.2 里 `ass_shaper.c` 直接 `#include <hb.h>`，连 `CONFIG_HARFBUZZ` 开关都没有），而 harfbuzz 的源码包 19 MB，在这个网络下下不动。0.14.0 还有 `CONFIG_HARFBUZZ` 条件编译，关掉照样能编，mpv 用到的那几个 API（`ass_set_shaper` / `ass_set_storage_size`）它都有；mpv 里调用 `ass_track_set_feature` 的两处本来就有 `#if LIBASS_VERSION >=` 保护，会自动跳过。

mpv 本体（`v0.35.1`，这是最后一个把 libplacebo 做成可选项的版本）：

```bash
meson setup build --cross-file cross.txt --buildtype=release --default-library=shared \
  -Dlibmpv=true -Dcplayer=false -Dtests=false -Dbuild-date=false \
  -Dgl=disabled -Dplain-gl=disabled -Dlibplacebo=disabled -Dvulkan=disabled \
  -Dshaderc=disabled -Dspirv-cross=disabled -Dd3d11=disabled -Ddirect3d=disabled \
  -Dd3d-hwaccel=disabled -Dcuda-hwaccel=disabled -Dlua=disabled -Djavascript=disabled \
  -Duchardet=disabled -Dlibbluray=disabled -Ddvdnav=disabled -Dcdda=disabled \
  -Dlibarchive=disabled -Dlibavdevice=disabled -Drubberband=disabled -Dlcms2=disabled \
  -Dzimg=disabled -Dzlib=disabled -Diconv=disabled -Djpeg=disabled -Dsixel=disabled \
  -Dvapoursynth=disabled -Dcaca=disabled -Dsdl2=disabled
```

mpv 0.35.1 配 ffmpeg 7 需要三处小补丁（都是 ffmpeg 7 的 API 删除导致的）：

1. `demux/demux_lavf.c`：`AVFormatContext.io_close` 被删了 → 改用 `io_close2`（返回 `int`，可以直接 `return avio_close(pb)`）。
2. `video/decode/vd_lavc.c`：`AVFrame.pkt_duration` → `duration`；`video/sws_utils.c`：`avcodec_enum_to_chroma_pos` → `av_chroma_location_enum_to_pos`。
3. `filters/f_swresample.c`：`in_channel_layout` / `out_channel_layout` 两个 64 位掩码选项被 `in_chlayout` / `out_chlayout`（`AVChannelLayout`）取代。**这一处不改的话 DLL 能编能链接，但一放歌就报 `libswresample failed to initialize`** ——`av_opt_set_int` 失败不报错，声道布局留空，`swr_init` 直接失败，现象是「播了 0.9 秒就结束、没有任何声音」。

还有两个 Windows 侧的小坑：`ksmedia.h` 用到 `WAVE_FORMAT_DOLBY_AC3_SPDIF` / `WAVE_FORMAT_DTS` 时 mingw 的 `mmreg.h` 还没进来（在 `ao_wasapi_utils.c` 里先 `#include <mmreg.h>`）；`-pthread` 会把 `libwinpthread-1.dll` 变成外部依赖，换成 `-Wl,-Bstatic -lwinpthread -Wl,-Bdynamic` 就只剩 `msvcrt.dll` + 系统 DLL。

编完 `strip --strip-all`：19 MB → **5.7 MB**（原版 122 MB）。导入表只剩 `KERNEL32 / USER32 / GDI32 / ole32 / oleaut32 / advapi32 / shell32 / winmm / dwmapi / avrt / bcrypt / msvcrt`，没有 Win8+ 的 `GetSystemTimePreciseAsFileTime` / `WaitOnAddress` / `ProcessPrng`。


## 构建

### Windows（原生）

```bat
cargo build --release
:: 产物：target\release\rust-music.exe
:: 把 libmpv-2.dll 放到同目录
```

### Windows（Linux 交叉编译）

```bash
rustup target add x86_64-pc-windows-gnu
# 需要 mingw 或 llvm-mingw 的 x86_64-w64-mingw32-gcc
cargo build --release --target x86_64-pc-windows-gnu
# 产物：target/x86_64-pc-windows-gnu/release/rust-music.exe
```

### Windows 7 兼容构建（已验证）

普通 `x86_64-pc-windows-gnu` 产物会导入 `GetSystemTimePreciseAsFileTime` / `WaitOnAddress` / `ProcessPrng`（Win8+ 才有），Win7 上直接报“无法定位程序输入点”。用 `target_vendor="win7"` 重编标准库即可：

```bash
rustup toolchain install nightly-2026-10-02
rustup component add rust-src --toolchain nightly-2026-10-02
rustup target add x86_64-pc-windows-gnu --toolchain nightly-2026-10-02
# 需要 /opt/llvm-mingw-20260922/bin 在 PATH 里
RUSTFLAGS='--cfg target_vendor="win7" -A explicit_builtin_cfgs_in_flags' \
  cargo +nightly-2026-10-02 build --release -Z build-std=std --target x86_64-pc-windows-gnu
# 校验（不应有输出）：
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/rust-music.exe \
  | grep -iE "GetSystemTimePreciseAsFileTime|WaitOnAddress|ProcessPrng|SetThreadDescription"
```

### Linux

```bash
sudo apt install libmpv2 libxkbcommon0
cargo run --release
```

## 歌词匹配怎么工作

对每个源：搜索候选 → 逐个打分 → 排序。任一个源的榜首 ≥ **75 分**（高置信度）就直接取用；都不够则记下全局最高分，若 ≥ **60 分**就退而求其次；再不行最后试一次 QQ「按歌名+歌手直取」（跳过搜索接口，绕开风控）。

打分（移植自 folia-major 的 `matchScore.ts`）：

- 标题 45 分、歌手 25 分、专辑 30 分
- 字符串相似度：完全相同 1.0；包含关系 `min/max` 长度比；否则字符集合 Jaccard
- 标题归一化会去掉 `feat./ft.` 和括号内容，但保留 `live / remix / instrumental / 伴奏` 等版本标记
- 歌手按 `, & 、 / feat 与` 拆分做 token 匹配，主歌手匹配有加成
- 时长倍率：≤1s `1.0`、≤3s `0.95`、≤5s `0.75`、≤10s `0.35`、更远 `0.1`
- **封顶**：歌手完全对不上（相似度 < 0.35）封顶 **68**；标题没命中或歌手/专辑都不可信封顶 **74**。这样「Lemon (粤语版) - 常颖杰」不会被当成原唱写进你的文件夹
- 低于 **60** 视为不可信，**不写入**

写入 `<音频文件名>.lrc`（统一转成增强 LRC 逐字格式）；有翻译时另存 `<音频文件名>.tlyric.lrc`。批量下载会跳过已有歌词的曲目。

> 受区域版权影响，个别歌曲（例如沙箱 IP 下的部分周杰伦原唱）搜不到，此时程序会判定“无可信匹配”而不是乱写。

## 目录结构

```
src/
  main.rs            入口、透明主窗口、中文字体加载
  app.rs             egui 界面、播放状态、桌面歌词窗口、设置
  mpv.rs             libmpv 动态绑定（libloading）
  library.rs         文件夹扫描、标签读取、内嵌歌词
  lyrics.rs          LRC 解析（标准/增强/逐字/offset/翻译）
  config.rs          配置持久化（歌词窗外观、源顺序、自动重扫）
  lyrics_online/
    mod.rs           源调度、阈值、保存、批量下载
    score.rs         匹配打分（标题/歌手/专辑/时长 + 封顶规则）
    convert.rs       QRC / KRC / TTML → 增强 LRC
    qq_crypt.rs      QQ QRC 的魔改 3DES 解密
    qq.rs / netease.rs / kugou.rs / amll.rs
  loudness.rs        等响度：K 加权测量、ReplayGain 标签、缓存、增益结算
  lyric_win32.rs     Windows 桌面歌词窗（分层窗口 + GDI 自绘，不依赖 GL 透明）
```

## 已知限制

- 桌面歌词鼠标穿透在 Windows 上稳定；Linux 取决于窗口管理器/合成器
- QQ 音乐搜索接口有风控，机房 IP 常返回 `code 2001`，代码里做了 3 次退避重试 + 「按名称直取」兜底，仍可能偶发失败
- 播放进度轮询刷新（播放 50ms / 暂停 100ms），未做 mpv 属性事件订阅
- 桌面歌词窗的拖动/透明只做了 Windows 实现；Linux 仍用 egui 子 viewport
- 逐字高亮取决于源：AMLL / QQ / 酷狗 有逐字，网易云通常只有逐行
