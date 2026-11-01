#!/usr/bin/env bash
#
# build_deb.sh —— 离线自包含 .deb（银河麒麟 V10 SP1 / x86_64 / glibc2.31）
#
# 背景：目标机离线、且可能未安装 libmpv/ffmpeg，无法用 apt 满足 Depends。
# 因此把“运行时非系统库”私有化到 /opt/haiwang-music/lib（绝不进 /usr/lib、/lib、
# /etc/ld.so*），Depends 仅保留 base 必装的 libc6 / libgcc-s1 —— 离线 dpkg -i 必成。
#
# 关键取舍：
#   - libmpv + ffmpeg 解码链 + X11/xcb/wayland + 字体栈 + 音频(alsa/jack)：
#     多数精简系统没有 → 全部随包内置（/opt/haiwang-music/lib，按 libmpv 完整闭包）
#   - libGL / libEGL / libGLX / libGLdispatch / libdrm / libgbm：
#     必须由目标机显卡驱动(风华/Mesa)提供 → 绝不打包（否则破坏驱动），走 Depends
#   - glibc / libgcc_s：由系统提供 → 绝不打包
#
# 结构：
#   /opt/haiwang-music/bin/rust-music              真实 ELF (755)
#   /opt/haiwang-music/lib/*.so*                   私有运行时库（含 libmpv.so.1 + .so.2 相对软链）
#   /opt/haiwang-music/usr/share/fonts/wqy-zenhei.ttc  CJK 字体（配合启动器 APPDIR 兜底）
#   /usr/bin/rust-music                            启动器 (sh, 755)：设 LD_LIBRARY_PATH/APPDIR 后 exec
#   /usr/share/applications|icons|doc/...          桌面集成与文档
#
# 对应交付规范：
#   1. 二进制 focal 编译(glibc2.31, 最高需求2.30)
#   2. 包内无 /usr/lib、/lib、/etc/ld.so*
#   3. 维护脚本仅应用级(刷新桌面/图标缓存)，无 ldconfig/服务重启/依赖安装
#   4. 依赖用 Depends 声明，不强装覆盖
#   5. 权限 755/644、无 setuid、Exec 绝对路径
#   6. Architecture=amd64，Version 递增
#   7. 脚本内置打包前后自查；建议另在干净机 dpkg -i/-r 演练
#   8. 软链全部相对且指向包内
#
set -euo pipefail

PKG="haiwang-music"
VERSION="0.1.0"
REV="2"                                   # -1=依赖版(需仓库)；-2=离线自包含版(本脚本)
ARCH="amd64"
APP_ROOT="/opt/haiwang-music"

BIN_SRC="${BIN_SRC:-/workspace/haiwang/artifacts/rust-music}"
ICON_SRC="${ICON_SRC:-/workspace/haiwang/assets/icon_256.png}"
APPIMAGE="${APPIMAGE:-/workspace/haiwang/artifacts/rust-music-x86_64.AppImage}"
OUT_DIR="${OUT_DIR:-/workspace/haiwang/artifacts}"

WORK="$(mktemp -d)"
STAGE="$WORK/${PKG}_${VERSION}-${REV}_${ARCH}"
DEB="$OUT_DIR/${PKG}_${VERSION}-${REV}_${ARCH}.deb"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

echo "==> [deb] 准备运行时库集（离线自包含核心）"
if [ -n "${LIB_SRC:-}" ] && [ -d "${LIB_SRC}" ]; then
  FONT_SRC="${FONT_SRC:?使用外部 LIB_SRC 时须同时指定 FONT_SRC}"
  echo "   使用外部库集: $LIB_SRC"
else
  echo "   未指定 LIB_SRC，回退从 AppImage 提取"
  mkdir -p "$WORK/appimg"
  cp "$APPIMAGE" "$WORK/appimg/app.AppImage"
  chmod +x "$WORK/appimg/app.AppImage"
  ( cd "$WORK/appimg" && ./app.AppImage --appimage-extract >/dev/null 2>&1 )
  LIB_SRC="$WORK/appimg/squashfs-root/usr/lib"
  FONT_SRC="$WORK/appimg/squashfs-root/usr/share/fonts/wqy-zenhei.ttc"
fi
[ -d "$LIB_SRC" ]   || { echo "!! 未找到库集: $LIB_SRC"; exit 1; }
[ -f "$FONT_SRC" ]  || { echo "!! 未找到字体: $FONT_SRC"; exit 1; }
echo "   库文件数: $(ls "$LIB_SRC" | wc -l) | 体积: $(du -sh "$LIB_SRC" | cut -f1)"

echo "==> [deb] 装配 $STAGE"
mkdir -p "$STAGE/DEBIAN" \
         "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE/usr/share/doc/$PKG" \
         "$STAGE$APP_ROOT/bin" "$STAGE$APP_ROOT/lib" "$STAGE$APP_ROOT/usr/share/fonts"

install -m 0755 "$BIN_SRC" "$STAGE$APP_ROOT/bin/rust-music"
cp -a "$LIB_SRC/." "$STAGE$APP_ROOT/lib/"          # 保留软链(RPATH 在 ELF 内部，随文件保留)
install -m 0644 "$FONT_SRC" "$STAGE$APP_ROOT/usr/share/fonts/wqy-zenhei.ttc"
install -m 0644 "$ICON_SRC" "$STAGE/usr/share/icons/hicolor/256x256/apps/$PKG.png"

# ---------- 启动器（应用级，无 setuid） ----------
cat > "$STAGE/usr/bin/rust-music" <<'EOF'
#!/bin/sh
# 启动器：私有库路径 + APPDIR(字体兜底) 后 exec 真实程序
APPDIR=/opt/haiwang-music
export APPDIR
LD_LIBRARY_PATH="$APPDIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LD_LIBRARY_PATH
exec "$APPDIR/bin/rust-music" "$@"
EOF
chmod 0755 "$STAGE/usr/bin/rust-music"

# ---------- desktop ----------
cat > "$STAGE/usr/share/applications/$PKG.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=RustMusic
Name[zh_CN]=海王音乐播放器
GenericName=Music Player
GenericName[zh_CN]=音乐播放器
Comment=Local music player powered by libmpv + egui (glow/OpenGL)
Comment[zh_CN]=基于 libmpv 与 egui 的本地音乐播放器（OpenGL 渲染）
Exec=/usr/bin/rust-music
Icon=$PKG
Terminal=false
Categories=AudioVideo;Audio;Player;
Keywords=music;player;audio;mp3;flac;
StartupWMClass=rust-music
EOF
chmod 0644 "$STAGE/usr/share/applications/$PKG.desktop"

# ---------- 文档 ----------
cat > "$STAGE/usr/share/doc/$PKG/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Haiwang-Music-Player
Source: https://github.com/744733778/Haiwang-Music-Player

Files: *
Copyright: 2026 744733778 and contributors
License: not-specified
 上游仓库未声明明确的许可证文件，请以仓库为准。
EOF
cat > "$STAGE/usr/share/doc/$PKG/changelog.Debian" <<EOF
$PKG ($VERSION-$REV) unstable; urgency=medium

  * Offline self-contained build: runtime libs (libmpv + ffmpeg chain) bundled
    under /opt/haiwang-music/lib; Depends reduced to libc6/libgcc-s1 so that
    dpkg -i succeeds on an offline Kylin host without any extra packages.
  * Launcher /usr/bin/rust-music sets LD_LIBRARY_PATH and APPDIR.
  * No /usr/lib, /lib or /etc/ld.so* files; no ldconfig/service actions.

 -- Haiwang Builder <builder@localhost>  $(date -R)
EOF
gzip -9 -n -c "$STAGE/usr/share/doc/$PKG/changelog.Debian" > "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"
rm -f "$STAGE/usr/share/doc/$PKG/changelog.Debian"
[ -f /tmp/Haiwang-Music-Player/README.md ] && install -m 0644 /tmp/Haiwang-Music-Player/README.md "$STAGE/usr/share/doc/$PKG/README.md" || true
[ -f /tmp/Haiwang-Music-Player/使用说明.txt ] && install -m 0644 "/tmp/Haiwang-Music-Player/使用说明.txt" "$STAGE/usr/share/doc/$PKG/使用说明.txt" || true

# ---------- 维护脚本（仅应用级缓存刷新） ----------
cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache   >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
exit 0
EOF
cat > "$STAGE/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache   >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
exit 0
EOF
chmod 0755 "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/postrm"

# ---------- control（离线：仅 base 必装依赖） ----------
INSTALLED_SIZE="$(du -sk --exclude=DEBIAN "$STAGE" | cut -f1)"
cat > "$STAGE/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION-$REV
Architecture: $ARCH
Maintainer: Haiwang Builder <builder@localhost>
Installed-Size: $INSTALLED_SIZE
Depends: libc6 (>= 2.30), libgcc-s1 | libgcc1, libgl1, libegl1, libdrm2, libgbm1
Section: sound
Priority: optional
Homepage: https://github.com/744733778/Haiwang-Music-Player
Description: 海王音乐播放器 (RustMusic) - 离线自包含 deb
 基于 Rust/egui 的本地音乐播放器，eframe glow(OpenGL) 后端，播放内核 libmpv。
 .
 本包为离线自包含版：libmpv 与完整 ffmpeg 解码链、CJK 字体已内置到
 /opt/haiwang-music，安装无需联网、无需额外依赖。
 libGL/libEGL 由目标机显卡驱动(风华/Mesa)提供；需在图形桌面会话中运行。
 已在 Ubuntu 20.04 (glibc 2.31) 编译，最高 GLIBC 需求 2.30，适配银河麒麟 V10 SP1。
EOF
chmod 0644 "$STAGE/DEBIAN/control"

# ---------- md5sums ----------
( cd "$STAGE" && find . -type f ! -path './DEBIAN/*' -printf '%P\n' | sort | while read -r f; do
    printf '%s  %s\n' "$(md5sum "$f" | cut -d' ' -f1)" "$f"
  done ) > "$STAGE/DEBIAN/md5sums"
chmod 0644 "$STAGE/DEBIAN/md5sums"

# ---------- 打包前断言 ----------
echo "==> [deb] 打包前断言"
fail=0
bad_paths=$(cd "$STAGE" && find . \( -path './usr/lib' -o -path './usr/lib/*' -o -path './lib' -o -path './lib/*' -o -path './etc/ld.so*' \) -print 2>/dev/null || true)
[ -z "$bad_paths" ] || { echo "!! 禁止的系统路径:"; echo "$bad_paths"; fail=1; }
if find "$STAGE" -perm /6000 -type f | grep -q .; then echo "!! 存在 setuid/setgid"; find "$STAGE" -perm /6000 -type f; fail=1; fi
[ -x "$STAGE/usr/bin/rust-music" ] || { echo "!! 启动器不可执行"; fail=1; }
grep -q '^Exec=/usr/bin/rust-music$' "$STAGE/usr/share/applications/$PKG.desktop" || { echo "!! Exec 非绝对路径"; fail=1; }
[ -x "$STAGE$APP_ROOT/bin/rust-music" ] || { echo "!! 真实程序不存在"; fail=1; }
[ -f "$STAGE$APP_ROOT/lib/libmpv.so.1" ] || { echo "!! 缺少 libmpv.so.1"; fail=1; }
[ -L "$STAGE$APP_ROOT/lib/libmpv.so.2" ] || { echo "!! 缺少 libmpv.so.2 软链"; fail=1; }
# /opt 私有库里绝不能混入 glibc 与显卡驱动入口库（其余(X11/xcb/字体/音频)允许内置）
for p in 'libc.so*' 'libm.so*' 'libpthread.so*' 'libdl.so*' 'librt.so*' 'libgcc_s.so*' 'libGL.so*' 'libEGL.so*' 'libGLX.so*' 'libGLdispatch.so*' 'libdrm.so*' 'libgbm.so*'; do
  if find "$STAGE$APP_ROOT/lib" -maxdepth 1 -name "$p" | grep -q .; then echo "!! /opt/lib 含禁止库: $p"; fail=1; fi
done
# 反向确认：X11 客户端库确实内置（保证精简桌面可运行）
[ -f "$STAGE$APP_ROOT/lib/libX11.so.6" ] || { echo "!! 未内置 libX11.so.6"; fail=1; }
# 所有软链必须相对
if find "$STAGE" -type l -lname '/*' | grep -q .; then echo "!! 存在绝对路径软链"; find "$STAGE" -type l -lname '/*'; fail=1; fi
[ "$fail" -eq 0 ] || { echo "!! 断言失败，中止"; exit 1; }
echo "   ✓ 无 /usr/lib、/lib、/etc/ld.so* 污染"
echo "   ✓ /opt/lib 无 glibc/显卡驱动库（libGL/libEGL/libGLX/libGLdispatch/libdrm/libgbm）"
echo "   ✓ X11 客户端库已内置；libmpv.so.1 + 相对软链 .so.2 就位"
echo "   ✓ 无 setuid；软链全为相对；Exec 绝对路径"

# ---------- 构建 ----------
echo "==> [deb] dpkg-deb --build"
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group "$STAGE" "$DEB"

# ---------- 复查 ----------
echo; echo "==> [deb] dpkg-deb -c 复查"
post_bad=$(dpkg-deb -c "$DEB" | awk '{print $NF}' | grep -E '^\./(usr/lib|lib|etc/ld\.so)' || true)
[ -z "$post_bad" ] || { echo "!! 包内存在禁止路径:"; echo "$post_bad"; exit 1; }
echo "   ✓ 无 /usr/lib、/lib、/etc/ld.so*"
echo "   ✓ 产物: $DEB"
ls -lh "$DEB" | awk '{print "   size:", $5}'
