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
| `artifacts/rust-music-x86_64.AppImage` | **主交付物**：可直接在银河麒麟上运行的 AppImage（约 90MB，已内置 libmpv/ffmpeg、GTK/XCB 依赖、中文字体）|
| `artifacts/haiwang-music_0.1.0-2_amd64.deb` | **deb 离线自包含安装包**（约 78MB）：libmpv+ffmpeg 全链、X11/xcb/wayland、字体栈、alsa/jack 等运行时库均内置到 `/opt/haiwang-music`；离线机 `dpkg -i` 即可装（Depends 仅需 glibc/libgcc/显卡驱动栈）|
| `artifacts/rust-music` | 编译出的原始 ELF（未打包 glibc，最高 GLIBC 需求 2.30 ≤ 2.31）|
| `artifacts/glibc_check.txt` | GLIBC 版本需求校验结论（兼容麒麟）|
| `assets/linuxdeploy-x86_64.AppImage` | 离线打包工具（linuxdeploy）|
| `assets/runtime-x86_64` | 离线 type2 runtime（appimagetool 打包用，免联网下载）|
| `assets/icon.png` / `icon_256.png` | 程序图标（256x256 为 AppImage 规范所需尺寸）|

> **兼容性结论**：主程序最高仅需求 `GLIBC_2.30`，低于麒麟 V10 SP1 的 `glibc 2.31`；AppImage **未打包 glibc**（运行时使用系统 glibc），符合发行版兼容目标。

---

## 在银河麒麟上运行

AppImage 需要 FUSE。详见 [`run-on-kylin.md`](./run-on-kylin.md)。要点：

```bash
# 1. 赋予执行权限
chmod +x rust-music-x86_64.AppImage

# 2. 若系统未装 FUSE，二选一：
#    a) 安装 fuse:  sudo apt install fuse
#    b) 用抽取模式运行:  ./rust-music-x86_64.AppImage --appimage-extract
#       然后执行解出的 squashfs-root/AppRun

# 3. 直接运行
./rust-music-x86_64.AppImage
```

若麒麟上已有 `libmpv`（多数桌面版自带），也可不依赖 AppImage 直接跑；若缺失或版本不对，AppImage 自带副本即可。

### 方式二：deb 离线自包含安装包（推荐用于离线 / 受管环境）

**无需联网、无需预装 libmpv**：`dpkg -i` 直接安装即可使用。

```bash
# 离线机直接安装（Depends 仅需 glibc/libgcc/显卡驱动栈，桌面机均已具备）
sudo dpkg -i haiwang-music_0.1.0-2_amd64.deb

# 也可用 apt（本地文件）
sudo apt install ./haiwang-music_0.1.0-2_amd64.deb

# 运行：应用菜单「海王音乐播放器」，或命令行
rust-music

# 卸载 / 彻底清除
sudo dpkg -r haiwang-music
sudo dpkg -P haiwang-music
```

**离线 deb 设计要点**：
- **运行时库私有化**：libmpv + 完整 ffmpeg 解码链、X11/xcb/wayland、fontconfig/freetype/harfbuzz、alsa/jack、SDL2/cdio/… 共 **280 个库**全部内置到 `/opt/haiwang-music/lib`，精简/离线桌面也能跑。
- **绝不污染系统库路径**：包内**不含任何 `/usr/lib`、`/lib`、`/etc/ld.so*`**（已 `dpkg-deb -c` 复查）；私有库位于 `/opt`，不覆盖任何系统包。
- **显卡驱动栈绝不打包**：`libGL/libEGL/libGLX/libGLdispatch/libdrm/libgbm` 必须由目标机驱动（风华/Mesa）提供，写入 `Depends`，打包反而会破坏显卡加速。
- **Depends 极简**：`libc6 (>= 2.30), libgcc-s1 | libgcc1, libgl1, libegl1, libdrm2, libgbm1` —— 均为桌面机自带，离线 `dpkg -i` 即成功。
- **启动器**：`/usr/bin/rust-music`（sh, 755）设置 `LD_LIBRARY_PATH=/opt/haiwang-music/lib` 与 `APPDIR`（供内置 CJK 字体兜底）后 `exec` 真身 `/opt/haiwang-music/bin/rust-music`。
- **维护脚本仅应用级**：postinst/postrm 只刷新桌面/图标缓存（缺失则跳过），**无 ldconfig、无服务重启、无依赖安装**。
- **权限/字段**：真身与启动器 755、文档/desktop/icon 644、无 setuid；`Architecture: amd64`；`Exec=/usr/bin/rust-music` 绝对路径；无 conffile。
- **软链**：仅 `libmpv.so.2 -> libmpv.so.1`（相对、包内）。
- **装前演练已做**：全新 ubuntu:20.04（glibc 2.31，同麒麟）仅装显卡栈、无 libmpv，`dpkg -i` → 运行 → `dpkg -r` → `dpkg -P` 全绿；`ldd` 确认 libmpv 依赖零缺口（除显卡驱动栈）。

---

## 目录结构

```
haiwang/
├── Dockerfile                  # 自包含复现构建镜像 (ubuntu:20.04 / glibc 2.31)
├── README.md                   # 本文件
├── run-on-kylin.md             # 麒麟上的运行/排障指南
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
    ├── rust-music-x86_64.AppImage
    ├── haiwang-music_0.1.0-2_amd64.deb
    ├── rust-music
    ├── glibc_check.txt
    └── *.log
```

---

## 复现构建

> 受限于网络/工具链，下面给出可复现的容器化方案（与本次构建一致）。

```bash
# 镜像基于 ubuntu:20.04 (glibc 2.31)，Rust 用 stable，源码 clone 多镜像兜底
docker build -t haiwang-builder -f Dockerfile .
docker run --rm -v "$PWD/out:/out" haiwang-builder
# 产物: out/rust-music-x86_64.AppImage
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
    libmpv1 libxcursor1 libxi6 libgl1 libegl1 libdrm2 libgbm1 fonts-wqy-zenhei
  bash /scripts/collect_libs.sh /out/libset /out/wqy-zenhei.ttc'   # 需挂载 scripts/
# 2) 用该库集装配离线 deb（宿主执行即可）
LIB_SRC="$PWD/out/libset" FONT_SRC="$PWD/out/wqy-zenhei.ttc" bash scripts/build_deb.sh
# 产物: artifacts/haiwang-music_0.1.0-2_amd64.deb
```

---

## 已知说明

- **AppImage**：主程序直接链接的库极少（仅 libdl/libgcc_s/libpthread/libm/libc），GUI 依赖（libX11、libGL、libEGL 等）由目标系统/显卡驱动提供 —— 这是 Linux 图形 AppImage 的标准做法；已内置 `libxkbcommon`、`libXcursor`、完整 mpv+ffmpeg 链与文泉驿中文字体。
- **deb（离线版）**：为保证离线可用，libmpv 的完整依赖（含 X11/xcb/wayland、字体栈、alsa/jack）已全部私有化到 `/opt/haiwang-music/lib`；仅 `libGL/libEGL/libGLX/libGLdispatch/libdrm/libgbm` 交由显卡驱动提供（写入 Depends）。因此它比 AppImage 更"自足"，在无 libmpv 的离线桌面上也能直接用。
- 风华显卡请确保已安装对应 OpenGL 驱动（Mesa 或厂商驱动），否则 eframe 的 glow 后端无法初始化。
