#!/usr/bin/env bash
#
# collect_libs.sh —— 在 Ubuntu 20.04 (focal, glibc 2.31) 内求 libmpv 的完整运行时闭包
#
# 目的：为离线 deb 准备“私有运行时库集”。只剔除必须由目标机显卡驱动/glibc 提供的库，
#       其余（X11/xcb/wayland/字体栈/音频 alsa+jack/usb/SDL2/cdio/…）全部收集，
#       保证在精简/离线桌面上 libmpv 与窗口层可加载。
#
# 前提（在 focal 容器内，先装齐运行时包）：
#   apt-get update && apt-get install -y --no-install-recommends \
#     libmpv1 libxcursor1 libxi6 libgl1 libegl1 libdrm2 libgbm1 fonts-wqy-zenhei
#
# 用法：
#   bash collect_libs.sh /out/libset [字体输出路径]
#   产出：/out/libset/*.so*（libmpv.so.1 真身 + libmpv.so.2 相对软链）
#         可选字体：将 wqy-zenhei.ttc 拷到第 2 个参数（默认不拷）
#
set -euo pipefail

OUT="${1:?用法: collect_libs.sh <outdir> [font-out]}"
FONT_OUT="${2:-}"
LDIR=/usr/lib/x86_64-linux-gnu

[ -e "$LDIR/libmpv.so.1" ] || { echo "!! 未找到 $LDIR/libmpv.so.1，请先 apt-get install libmpv1"; exit 1; }

rm -rf "$OUT"; mkdir -p "$OUT"

echo "==> 收集 libmpv 及窗口层依赖闭包"
LIST=/tmp/_libmpv_list.txt
: > "$LIST"
echo "$LDIR/libmpv.so.1" >> "$LIST"
ldd "$LDIR/libmpv.so.1" | awk '/=> \// {print $3}' >> "$LIST"
# 窗口层(winit)通过 dlopen 使用、libmpv 未必 NEEDED 的库，显式补上
for l in libX11.so.6 libXcursor.so.1 libXrandr.so.2 libXi.so.6 \
         libxkbcommon.so.0 libwayland-client.so.0 libwayland-cursor.so.0; do
  [ -e "$LDIR/$l" ] || continue
  echo "$LDIR/$l" >> "$LIST"
  ldd "$LDIR/$l" 2>/dev/null | awk '/=> \// {print $3}' >> "$LIST"
done
sort -u "$LIST" | grep '^/' > /tmp/_libmpv_all.txt

echo "==> 过滤：剔除 glibc / libgcc_s / 显卡驱动栈，其余复制"
while read -r p; do
  b=$(basename "$p")
  case "$b" in
    libc.so.*|libm.so.*|libpthread.so.*|libdl.so.*|librt.so.*|libresolv.so.*|libnsl.so.*|libutil.so.*|ld-linux*) continue;;
    libgcc_s.so.*) continue;;
    libGL.so.*|libEGL.so.*|libGLX.so.*|libGLdispatch.so.*|libdrm.so.*|libgbm.so.*) continue;;
  esac
  cp -L "$p" "$OUT/$b"
done < /tmp/_libmpv_all.txt

# 确保 libmpv 真身与 .so.2 相对软链
cp -L "$LDIR/libmpv.so.1" "$OUT/libmpv.so.1"
ln -sf libmpv.so.1 "$OUT/libmpv.so.2"

echo "==> 结果"
echo "   库文件数: $(ls "$OUT" | wc -l) | 体积: $(du -sh "$OUT" | cut -f1)"
echo "   系统需提供(未随包): "
{ ldd "$LDIR/libmpv.so.1" | awk '/=> \//{print $1}'; } | sort -u > /tmp/_need.txt
ls "$OUT" | sort -u > /tmp/_have.txt
comm -23 /tmp/_need.txt /tmp/_have.txt | sed 's/^/     - /'

if [ -n "$FONT_OUT" ] && [ -f /usr/share/fonts/truetype/wqy/wqy-zenhei.ttc ]; then
  cp /usr/share/fonts/truetype/wqy/wqy-zenhei.ttc "$FONT_OUT"
  echo "   字体已拷贝: $FONT_OUT"
fi
