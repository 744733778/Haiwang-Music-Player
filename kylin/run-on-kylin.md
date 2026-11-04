# 在银河麒麟 V10 SP1 上运行海王音乐播放器

本文档针对 **银河麒麟 V10 SP1 (x86_64, glibc 2.31, 风华显卡走 OpenGL)** 说明如何运行交付的 AppImage 或 deb，以及常见问题的排查。

---

## 零、用 deb 安装（推荐，离线环境首选）

本 deb 为**离线自包含版**：libmpv+ffmpeg 全链、X11/音频/字体等运行时库已内置到
`/opt/hwplayer`，**无需联网、无需预装 libmpv**。

```bash
# 从旧版（haiwang-music）升级：两个包互斥，必须先卸旧包
sudo dpkg -r haiwang-music 2>/dev/null || true

# 离线直接安装（Depends 仅 glibc/libgcc/显卡驱动栈，桌面机均已具备）
sudo dpkg -i hwplayer_0.1.0-4_amd64.deb

# 运行：应用菜单里选「hwplayer」，或命令行
hwplayer

# 卸载 / 彻底清除
sudo dpkg -r hwplayer
sudo dpkg -P hwplayer
```

> 程序已更名为 **hwplayer**（命令、菜单项、窗口标题）。配置文件会自动沿用旧版
> `~/.config/rust-music/config.json`，已导入的音乐文件夹和音量不会丢。

- 安装内容：`/opt/hwplayer/`（程序 + 私有库 + 字体）、`/usr/bin/hwplayer`(启动器)、desktop、图标、文档；**不向 `/usr/lib`、`/lib`、`/etc/ld.so*` 写入任何文件**，不做 ldconfig/服务操作。
- 若 `dpkg -i` 提示缺 `libgl1/libegl1/libdrm2/libgbm1`：说明该机是精简系统、未装显卡 GL 栈。这类机器本来也无法渲染界面；请先装好显卡驱动（风华/Mesa），或改用 AppImage。
- 若系统缺 libmpv **不影响**本 deb —— libmpv 已随包内置（`/opt/hwplayer/lib`）。

### 选择音乐文件夹

点顶部「浏览…」即可，不必手敲路径：

- **有 `zenity` 时**：直接弹出系统文件选择器（GTK 原生对话框）。推荐装上以获得最佳体验：
  `sudo apt install zenity`（deb 里只是 `Recommends`，不装也能正常用）
- **没有 `zenity`/`kdialog` 时**：自动弹出内置目录浏览器（可「⬆ 上级」浏览、点「选择此文件夹」确认）。
- 也可以直接在输入框粘贴路径后按 **回车**；支持 `~/音乐`、带引号的路径、`file://` URI。
- 还可以把文件夹**直接拖进窗口**导入。

---

## 一、快速运行（AppImage）

```bash
# 进入产物目录
cd /path/to/hwplayer-x86_64.AppImage

# 1. 给执行权限
chmod +x hwplayer-x86_64.AppImage

# 2. 直接运行（推荐，需 FUSE）
./hwplayer-x86_64.AppImage
```

首次运行会把它自身挂载到临时目录（`$APPDIR`），自动设置 `LD_LIBRARY_PATH` 与字体兜底路径。

---

## 二、FUSE 相关问题（最常见）

AppImage 依赖 FUSE 把自身挂载为只读文件系统。

- **现象**：运行报错 `fuse: ...` / `Cannot mount AppImage, please install FUSE` / `AppImages require FUSE`。
- **解决（二选一）**：
  1. 安装 FUSE：
     ```bash
     sudo apt update && sudo apt install fuse
     # 若提示 fuse3 与 fuse 冲突，按提示处理即可
     ```
  2. 用**抽取模式**运行（无需 FUSE）：
     ```bash
     ./hwplayer-x86_64.AppImage --appimage-extract
     # 生成 squashfs-root/，直接执行:
     ./squashfs-root/AppRun
     ```

> 提示：抽取模式会把程序解到当前目录，可长期保留 `squashfs-root/` 作为"免 FUSE"的安装方式。

---

## 三、显卡 / OpenGL（风华显卡）

本程序用 **egui/eframe 的 glow（OpenGL）后端**，不走 Vulkan/wgpu。

- **前提**：系统需有可用的 OpenGL（Mesa 或风华显卡厂商驱动）。
- 若启动时窗口黑屏/崩溃且无显示，多半是 OpenGL 上下文创建失败：
  ```bash
  # 验证 OpenGL 是否可用
  glxinfo | grep "OpenGL version"     # 需安装 mesa-utils
  ```
- 确保风华显卡驱动已正确安装并启用；若用软件渲染兜底，可设置：
  ```bash
  export LIBGL_ALWAYS_SOFTWARE=1      # 仅用于验证, 性能差, 正式使用应走硬件 GL
  ```

---

## 四、音频 / libmpv

播放内核是 **libmpv**，AppImage 已**内置 libmpv + 完整 ffmpeg 解码链**，因此不依赖系统是否装了 mpv。

- 若你偏好使用系统 libmpv，可正常安装：
  ```bash
  sudo apt install libmpv1
  ```
- 程序加载顺序：`libmpv.so.2` → 回退 `libmpv.so.1`。AppImage 内已放 `libmpv.so.1` 并建立 `libmpv.so.2` 软链，无需系统提供。

---

## 五、中文字体

AppImage 已内置 **文泉驿正黑 (wqy-zenhei)**，并通过 `$APPDIR/usr/share/fonts/wqy-zenhei.ttc` 兜底路径读取，保证歌名/歌词中文不乱码。

- 若你解包到非 FUSE 环境手动运行（`squashfs-root/AppRun`），只要目录结构保持 `$APPDIR/usr/share/fonts/...` 不变，字体仍生效。
- 若仍乱码，确认系统已安装 CJK 字体之一（程序还会回退到
  `/usr/share/fonts/.../NotoSansCJK-Regular.ttc` 等系统路径）：
  ```bash
  sudo apt install fonts-wqy-zenhei fonts-noto-cjk
  ```

---

## 六、兼容性说明

- **glibc**：主程序最高需求 `GLIBC_2.30`，低于麒麟的 `glibc 2.31`，无需担心。
- AppImage **未打包 glibc**（使用系统 glibc），也未打包 `libGL/libEGL`（由显卡驱动提供）——这是正确且必要的。
- 已内置：`libmpv`+ffmpeg 解码链、`libxkbcommon`、`libXcursor`、GTK/XCB 相关界面库、文泉驿中文字体。

---

## 七、排障速查

| 现象 | 可能原因 | 处理 |
|------|----------|------|
| `Cannot mount AppImage, please install FUSE` | 缺 FUSE | 装 `fuse`，或 `--appimage-extract` 后跑 `AppRun` |
| 启动即退，报 `XOpenDisplayFailed` | 无 X 显示 / 在非桌面环境 | 确保在麒麟桌面会话内运行；检查 `echo $DISPLAY` |
| 窗口黑屏、GL 初始化失败 | OpenGL/风华驱动异常 | 用 `glxinfo` 验证；确认显卡驱动已启用 |
| 无声音 | 音频后端/输出设备 | 确认系统音频正常；libmpv 已内置 |
| 中文乱码 | 字体缺失 | 用内置 wqy-zenhei；或装 `fonts-noto-cjk` |
| 报缺某个 `.so` | 极少见，依赖收集遗漏 | 用抽取模式并在 `squashfs-root/usr/lib` 核查；反馈给我们 |

如仍有问题，把启动时的终端报错贴出即可定位。
