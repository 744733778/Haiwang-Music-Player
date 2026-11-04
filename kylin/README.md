# 海王音乐播放器 · 银河麒麟适配版

为 **银河麒麟 V10 SP1 (x86_64, glibc 2.31, 风华显卡走 OpenGL)** 编译的可运行版本。

- 上游仓库：[github.com/744733778/Haiwang-Music-Player](https://github.com/744733778/Haiwang-Music-Player)
- 图形后端：**egui / eframe `glow` (OpenGL)** —— 不依赖 wgpu/Vulkan，适配风华显卡
- 播放内核：**libmpv**（运行时动态加载，已随包内置 ffmpeg 解码链）
- 渲染验证：在本环境解包 + 加载冒烟测试通过（仅因无显示器报 `XOpenDisplayFailed`，所有内嵌动态库均能解析）

---

## 产物

| 文件 | 说明 |
|------|------|
| `artifacts/hwplayer-x86_64.AppImage` | **主交付物**：可直接在银河麒麟上运行的 AppImage（约 87MB，已内置 libmpv/ffmpeg、GTK/XCB 依赖、中文字体）|
| `artifacts/hwplayer_0.1.0-4_amd64.deb` | **deb 离线自包含安装包**（约 78MB）：libmpv+ffmpeg 全链、X11/xcb/wayland、字体栈、alsa/jack 等运行时库均内置到 `/opt/hwplayer`；离线机 `dpkg -i` 即可装（Depends 仅需 glibc/libgcc/显卡驱动栈）|
| `artifacts/hwplayer` | 编译出的原始 ELF（未打包 glibc，最高 GLIBC 需求 2.30 ≤ 2.31）|
| `artifacts/glibc_check.txt` | GLIBC 版本需求校验结论（兼容麒麟）|
| `assets/linuxdeploy-x86_64.AppImage` | 离线打包工具（linuxdeploy）|
| `assets/runtime-x86_64` | 离线 type2 runtime（appimagetool 打包用，免联网下载）|
| `assets/icon.png` / `icon_256.png` | 程序图标（256x256 为 AppImage 规范所需尺寸）|

> **版本说明**：`0.1.0-4` 修复了「**一首播完不会自动播下一首**」，并把程序整体更名为 **hwplayer**（命令、包名、desktop、窗口标题）；同时修掉了 AppImage 打包链路里三个会让它在真实桌面起不来的问题。详见下方[更新记录](#更新记录)。

> **兼容性结论**：主程序最高仅需求 `GLIBC_2.30`，低于麒麟 V10 SP1 的 `glibc 2.31`；AppImage **未打包 glibc**（运行时使用系统 glibc），符合发行版兼容目标。

---

## 在银河麒麟上运行

AppImage 需要 FUSE。详见 [`run-on-kylin.md`](./run-on-kylin.md)。要点：

```bash
# 1. 赋予执行权限
chmod +x hwplayer-x86_64.AppImage

# 2. 若系统未装 FUSE，二选一：
#    a) 安装 fuse:  sudo apt install fuse
#    b) 用抽取模式运行:  ./hwplayer-x86_64.AppImage --appimage-extract
#       然后执行解出的 squashfs-root/AppRun

# 3. 直接运行
./hwplayer-x86_64.AppImage
```

若麒麟上已有 `libmpv`（多数桌面版自带），也可不依赖 AppImage 直接跑；若缺失或版本不对，AppImage 自带副本即可。

### 方式二：deb 离线自包含安装包（推荐用于离线 / 受管环境）

**无需联网、无需预装 libmpv**：`dpkg -i` 直接安装即可使用。

```bash
# 离线机直接安装（Depends 仅需 glibc/libgcc/显卡驱动栈，桌面机均已具备）
sudo dpkg -i hwplayer_0.1.0-4_amd64.deb

# 也可用 apt（本地文件）
sudo apt install ./hwplayer_0.1.0-4_amd64.deb

# 运行：应用菜单「海王音乐播放器」，或命令行
hwplayer

# 卸载 / 彻底清除
sudo dpkg -r hwplayer
sudo dpkg -P hwplayer
```

**离线 deb 设计要点**：
- **运行时库私有化**：libmpv + 完整 ffmpeg 解码链、X11/xcb/wayland、fontconfig/freetype/harfbuzz、alsa/jack、SDL2/cdio/… 共 **282 个库**全部内置到 `/opt/hwplayer/lib`，精简/离线桌面也能跑。
- **绝不污染系统库路径**：包内**不含任何 `/usr/lib`、`/lib`、`/etc/ld.so*`**（已 `dpkg-deb -c` 复查）；私有库位于 `/opt`，不覆盖任何系统包。
- **显卡驱动栈绝不打包**：`libGL/libEGL/libGLX/libGLdispatch/libdrm/libgbm` 必须由目标机驱动（风华/Mesa）提供，写入 `Depends`，打包反而会破坏显卡加速。
- **Depends 极简**：`libc6 (>= 2.30), libgcc-s1 | libgcc1, libgl1, libegl1, libdrm2, libgbm1` —— 均为桌面机自带，离线 `dpkg -i` 即成功。
- **启动器**：`/usr/bin/hwplayer`（sh, 755）设置 `LD_LIBRARY_PATH=/opt/hwplayer/lib` 与 `APPDIR`（供内置 CJK 字体兜底）后 `exec` 真身 `/opt/hwplayer/bin/hwplayer`。
- **文件夹选择**：优先调用系统自带文件选择器（`zenity` / `kdialog` / `yad` / `qarma`，子进程方式，零链接依赖）；`Recommends: zenity` 只作建议、离线 `dpkg -i` 不会因缺它失败；若系统一个都没有，自动退到内置 egui 目录浏览器，**保证任何环境都能选目录**。
- **维护脚本仅应用级**：postinst/postrm 只刷新桌面/图标缓存（缺失则跳过），**无 ldconfig、无服务重启、无依赖安装**。
- **权限/字段**：真身与启动器 755、文档/desktop/icon 644、无 setuid；`Architecture: amd64`；`Exec=/usr/bin/hwplayer` 绝对路径；无 conffile。
- **软链**：仅 `libmpv.so.2 -> libmpv.so.1`（相对、包内）。
- **装前演练已做**：全新 ubuntu:20.04（glibc 2.31，同麒麟）仅装显卡栈、无 libmpv，`dpkg -i` → 运行 → `dpkg -r` → `dpkg -P` 全绿；`ldd` 确认 libmpv 依赖零缺口（除显卡驱动栈）。

---

## 目录结构

```
haiwang/
├── Dockerfile                  # 自包含复现构建镜像 (ubuntu:20.04 / glibc 2.31)
├── README.md                   # 本文件
├── run-on-kylin.md             # 麒麟上的运行/排障指南
├── folder-picker.patch         # 本次「系统原生文件夹选择器」改造的源码补丁（可 git apply）
├── assets/                     # 离线构建资产
│   ├── linuxdeploy-x86_64.AppImage
│   ├── runtime-x86_64
│   ├── icon.png  (1024)
│   └── icon_256.png
├── scripts/
│   ├── build.sh                # [1] 在 ubuntu:20.04 内 装依赖→装Rust→clone→字体补丁→编译→GLIBC校验
│   ├── build_tail.sh           # 源码已就位时的续跑脚本(跳过 clone)
│   ├── package.sh              # [2] linuxdeploy 部署 AppDir + 本地 runtime 打包 AppImage
│   ├── collect_libs.sh         # [3] 在 focal 内求 libmpv 完整闭包(离线 deb 的私有库集)
│   ├── build_deb.sh            # [4] 装配离线 deb（/opt 私有库 + 打包前后自查断言）
│   └── patch_eframe.py         # (备查) 曾尝试彻底移除 egui-wgpu 的方案，最终因 eframe 需 edition2024 未采用
└── artifacts/                  # 构建产物与日志
    ├── hwplayer-x86_64.AppImage
    ├── hwplayer_0.1.0-4_amd64.deb
    ├── hwplayer
    ├── glibc_check.txt
    └── *.log
```

---

## 更新记录

### 0.1.0-4（当前）

**问题 1：一首播完不会自动播下一首（体验中断）**

- 根因：`src/mpv.rs` 里 mpv 配了 `keep-open=no`。文件播完时 mpv 会卸载文件、核心进入 idle，`eof-reached` 属性随文件一起失效，`poll_mpv()` 轮询到的永远是 false —— UI 根本不知道「播完了」，自然不会调用 `next()`。
- 修复：
  - 改为 `keep-open=yes`（播完保留文件并自动暂停）；
  - 结束判据改为**进度到结尾 + mpv 已自动暂停**（`dur > 0.5 && pos >= dur - 0.2 && paused`），同时兼容 `END_FILE` 事件（部分 libmpv 版本会发）；
  - 加「每曲只触发一次」标志 + 600ms 切歌保护窗，避免 loadfile 异步切换时把上一曲的收尾状态误判成新曲播完而连锁跳歌；
  - 事件泵只给播放实例启用；响度分析用的 `wait_end_file` 实例不启用，避免两个线程抢同一个事件队列。
- 验证：3 首 10 秒音频，34 秒内严格按 `01→02→03→01` 切歌，每首播满 10 秒（`[advance]` 日志逐条核对），无跳歌、无重复切。

**问题 2：程序更名为 hwplayer**

- 包名 `haiwang-music` → `hwplayer`，并声明 `Provides/Conflicts/Replaces: haiwang-music`（与旧包互斥，升级前需先卸旧包）；
- 可执行 `/usr/bin/hwplayer`、真身 `/opt/hwplayer/bin/hwplayer`、`hwplayer.desktop`、窗口标题与 app_id 均为 `hwplayer`；
- 配置目录优先 `~/.config/hwplayer`；若新目录没有配置而旧 `~/.config/rust-music` 存在，则继续沿用旧文件，**老用户不会丢已导入的音乐文件夹和音量设置**。

**同时修掉的 AppImage 打包问题（否则真实桌面起不来）**

| 问题 | 后果 | 处理 |
|---|---|---|
| `AppDir/AppRun` 是指向二进制的软链 | `cat > AppRun` 顺着软链把 8.6MB 真二进制覆盖成 238 字节脚本 | 写入前先 `rm -f AppRun` |
| linuxdeploy 的 AppRun 不设 `LD_LIBRARY_PATH` | 运行时 `dlopen(libxkbcommon-x11.so.0)` 失败 → 启动即 panic | 改用自写 AppRun 脚本，设 `APPDIR` + `LD_LIBRARY_PATH` |
| linuxdeploy 把 `libasound/libjack/libusb` 当系统库跳过 | libmpv 起不来，界面红字报 `libmpv.so: cannot open shared object file` | 用完整依赖闭包（282 个库）补齐 `usr/lib` |

### 0.1.0-3

**问题 1：Linux 版没有「选择文件夹」按钮，手输路径不识别**

- 根因：`src/app.rs` 里 `pick_folder()` 与「浏览…」按钮整体被 `#[cfg(windows)]` 包裹，Linux 构建时被裁掉；输入框又只做了 `trim()`，`~/音乐`、带引号路径、`file://` URI 一律 `is_dir()` 失败。
- 修复：
  - 「浏览…」跨平台启用，Linux 走**系统自带文件选择器**（`zenity` → `kdialog` → `yad` → `qarma` 依次尝试），子进程调用，不链接 GTK/Qt，不破坏 `/opt` 私有库隔离。
  - 发现并规避 `zenity` 的隐蔽坑：locale 非 UTF-8 时中文 `--title` 会被判为非法选项直接退出 → 非 UTF-8 环境给子进程注入 `LC_ALL=C.UTF-8`；已是 UTF-8（麒麟默认 `zh_CN.UTF-8`）则原样继承，保持中文界面。
  - 系统一个选择器都没有时，自动弹出**内置 egui 目录浏览器**兜底（可上级导航、点选确认），任何环境都能选目录。
  - 输入框增强：支持 `~` 展开、成对引号剥离、`file://` URI 解码；**回车直接导入**。
  - 新增：可直接把文件夹**拖进窗口**导入。
- 验证：Xvfb 下模拟点击「浏览…」→ zenity 窗口弹出 → 回车确认 → 路径写入 `config.json`；移除 zenity 后 → 内置浏览器弹出 → 点「选择此文件夹」→ 路径写入成功。

**问题 2：离线安装后启动 panic（`libxkbcommon-x11.so could not be loaded`）**

- 根因：winit 通过 `xkbcommon-dl` 在**建立 X 连接之后**才 `dlopen("libxkbcommon-x11.so.0")`，该库不在 ELF 的 `NEEDED` 里，`ldd` 与「无 X 服务器」的冒烟测试都发现不了；之前的离线测试因为没有显示服务器，程序在更早阶段就退出了，恰好掩盖了它。
- 修复：把 `libxkbcommon-x11.so.0` 与其依赖 `libxcb-xkb.so.1` 纳入私有库集（deb）与 AppImage（`linuxdeploy --library` 显式收集）；`collect_libs.sh` 同步固化。
- 验证：在只装显卡栈、无 libmpv/zenity 的全新 focal 容器里，用 `LD_DEBUG=libs` 启动，**零 dlopen 失败**，进程稳定存活。

---

## 复现构建

> 受限于网络/工具链，下面给出可复现的容器化方案（与本次构建一致）。

```bash
# 镜像基于 ubuntu:20.04 (glibc 2.31)，Rust 用 stable，源码 clone 多镜像兜底
docker build -t haiwang-builder -f Dockerfile .
docker run --rm -v "$PWD/out:/out" haiwang-builder
# 产物: out/hwplayer-x86_64.AppImage
```

### 关键约束（为何这样配）

1. **基镜像 = ubuntu:20.04 (glibc 2.31)**：二进制 glibc 由 OS 决定，与 Rust 版本无关，据此对齐麒麟。
2. **Rust 用 `stable`**：仓库 `Cargo.toml` 写 `rust-version=1.82` 已失真；egui/eframe 0.32.3 栈需 edition2024（Rust≥1.85），传递依赖（lofty/image/icu 等）要求高达 rustc 1.89。1.82、1.85 实测均不可行，故用 stable（实测 rustc 1.99）。
3. **`glow` 后端已写死在 Cargo.toml**：直接 `cargo build --release`，无需也不该传 `--features glow`。
4. **字体兜底补丁**：`src/main.rs` 字体候选列表新增 `$APPDIR/usr/share/fonts/wqy-zenhei.ttc`，保证 AppImage 离线/麒麟上中文不乱码（原系统路径兜底保留）。
5. **libmpv 运行时 dlopen**：程序先试 `libmpv.so.2` 再回退 `.so.1`，故 `package.sh` 用 `--library libmpv.so.1` 收集并建 `.so.2` 软链，打包 ffmpeg 解码链。
6. **不打包 glibc / 不打包 libGL、libEGL**：glibc 由系统提供（兼容性前提）；`libGL/libEGL` 必须由目标机的显卡驱动（风华/Mesa）提供，打包反而会破坏显卡加速。

### 构建离线 deb

```bash
# 1) 在 focal 容器内求 libmpv 完整闭包（剔除 glibc/显卡驱动栈）
docker run --rm -v "$PWD/out:/out" ubuntu:20.04 bash -c '
  apt-get update -qq && apt-get install -y --no-install-recommends \
    libmpv1 libxcursor1 libxi6 libgl1 libegl1 libdrm2 libgbm1 \
    libxkbcommon-x11-0 fonts-wqy-zenhei
  bash /scripts/collect_libs.sh /out/libset /out/wqy-zenhei.ttc'   # 需挂载 scripts/
# 2) 用该库集装配离线 deb（宿主执行即可）
LIB_SRC="$PWD/out/libset" FONT_SRC="$PWD/out/wqy-zenhei.ttc" bash scripts/build_deb.sh
# 产物: artifacts/hwplayer_0.1.0-4_amd64.deb
```

---

## 已知说明

- **AppImage**：主程序直接链接的库极少（仅 libdl/libgcc_s/libpthread/libm/libc），GUI 依赖（libX11、libGL、libEGL 等）由目标系统/显卡驱动提供 —— 这是 Linux 图形 AppImage 的标准做法；已内置 `libxkbcommon`、**`libxkbcommon-x11`**、`libXcursor`、完整 mpv+ffmpeg 链与文泉驿中文字体。
- **deb（离线版）**：为保证离线可用，libmpv 的完整依赖（含 X11/xcb/wayland、字体栈、alsa/jack）已全部私有化到 `/opt/hwplayer/lib`；仅 `libGL/libEGL/libGLX/libGLdispatch/libdrm/libgbm` 交由显卡驱动提供（写入 Depends）。因此它比 AppImage 更"自足"，在无 libmpv 的离线桌面上也能直接用。
- 风华显卡请确保已安装对应 OpenGL 驱动（Mesa 或厂商驱动），否则 eframe 的 glow 后端无法初始化。
