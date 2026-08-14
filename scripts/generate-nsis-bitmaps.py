from PIL import Image, ImageDraw, ImageFont
from pathlib import Path

# 锚定脚本位置而非 CWD：从错误目录运行时，相对路径会静默建目录、把产物写错地方。
OUT_DIR = Path(__file__).resolve().parent.parent / "app" / "src-tauri" / "icons"
OUT_DIR.mkdir(parents=True, exist_ok=True)

PANEL_TOP = "#1d1d20"
PANEL_BOTTOM = "#0c0c0d"
ICON_PANEL = "#151517"
ICON_STROKE = "#2a2a2d"
DOT_GREEN = "#4ec9a5"
DOT_AMBER = "#e0a23c"
TEXT_COLOR = "#f5f5f7"
SUBTEXT_COLOR = "#a0a0a8"
# 一键安装背景的分区底色。改这两个值必须同步 installer.nsi 里 SetCtlColors 的
# 对应色值（MEOWO 标记块内有同名注释），否则控件文字会带色块。
BG_BASE = "#17171a"
BG_BAND = "#101012"
BG_GLOW = "#19191c"  # 与底色只差一档：实拍中硬边圆对比稍大就显廉价


def interpolate(c1, c2, t):
    t = max(0, min(1, t))
    def parse(c):
        return tuple(int(c[i:i+2], 16) for i in (1, 3, 5))
    a, b = parse(c1), parse(c2)
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def gradient_bg(size):
    img = Image.new("RGB", size)
    draw = ImageDraw.Draw(img)
    w, h = size
    for y in range(h):
        t = y / (h - 1) if h > 1 else 0
        draw.line([(0, y), (w, y)], fill=interpolate(PANEL_TOP, PANEL_BOTTOM, t))
    return img


def load_font(name, size):
    candidates = [
        Path("/c/Windows/Fonts") / name,
        Path("C:/Windows/Fonts") / name,
    ]
    for p in candidates:
        if p.exists():
            return ImageFont.truetype(str(p), size)
    return ImageFont.load_default()


def paste_dot(img, cx, cy, r, color):
    """Draw a flat dot on an RGB image."""
    draw = ImageDraw.Draw(img)
    draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=color)


def make_header():
    w, h = 150, 57
    img = gradient_bg((w, h))
    draw = ImageDraw.Draw(img)

    icon_size = 28
    ix = w - 14 - icon_size
    iy = (h - icon_size) // 2
    draw.rounded_rectangle(
        [ix, iy, ix + icon_size, iy + icon_size],
        radius=7,
        fill=ICON_PANEL,
        outline=ICON_STROKE,
        width=1,
    )
    dot_r = 5
    dot_cy = iy + icon_size // 2 - 2
    paste_dot(img, ix + icon_size // 2 - 5, dot_cy, dot_r, DOT_GREEN)
    paste_dot(img, ix + icon_size // 2 + 5, dot_cy, dot_r, DOT_AMBER)

    font = load_font("segoeui.ttf", 15)
    draw.text((14, (h - 15) // 2 + 1), "Meowo", fill=TEXT_COLOR, font=font)

    img.save(OUT_DIR / "nsis-header.bmp", "BMP")


def make_sidebar():
    w, h = 164, 314
    img = gradient_bg((w, h))
    draw = ImageDraw.Draw(img)

    icon_size = 80
    ix = (w - icon_size) // 2
    iy = 48
    draw.rounded_rectangle(
        [ix, iy, ix + icon_size, iy + icon_size],
        radius=icon_size // 5,
        fill=ICON_PANEL,
        outline=ICON_STROKE,
        width=1,
    )

    cx = ix + icon_size // 2
    cy = iy + icon_size // 2 - 10
    paste_dot(img, cx - 17, cy, 17, DOT_GREEN)
    paste_dot(img, cx + 17, cy, 17, DOT_AMBER)

    title_font = load_font("segoeui.ttf", 17)
    sub_font = load_font("NotoSansSC-VF.ttf", 11)
    draw.text((w // 2, iy + icon_size + 24), "Meowo", fill=TEXT_COLOR, font=title_font, anchor="mm")
    draw.text((w // 2, iy + icon_size + 48), "AI 会话看板", fill=SUBTEXT_COLOR, font=sub_font, anchor="mm")

    img.save(OUT_DIR / "nsis-sidebar.bmp", "BMP")


def make_oneclick_bg():
    """一键安装器的全屏背景（逻辑 780x480 的 2x，安装器端 LoadImage 缩小采样）。

    刻意用**纯平色分区**而非渐变：NSIS 原生控件（Label/CheckBox）的文字背景只能靠
    SetCtlColors 配一个纯色，控件落在哪个区，就配哪个区的底色，才能做到无缝。
    文字（多语言的 tagline / 勾选项）一律不烙图，由安装器用系统字体渲染；
    这里只烙语言无关的品牌视觉（图标 + "Meowo" 字标）。
    分区（与 installer.nsi 的控件布局按同一比例对齐）：
      0%..75%   主区   BG_BASE   —— 品牌区 + 安装按钮 + 进度条都在这里
      75%..100% 底带   BG_BAND   —— 「自定义安装」展开区的控件落位
    """
    w, h = 1560, 960
    img = Image.new("RGB", (w, h), BG_BASE)
    draw = ImageDraw.Draw(img)

    # 顶部两角的暗色大圆，给纯色底一点纵深。**只允许出现在 y<36% 的品牌区**：
    # 36%..75% 是 tagline/升级提示/安装按钮/进度条的控件落位带，SetCtlColors 配的是
    # BG_BASE 纯色，装饰圆伸进来会在控件矩形边缘露出色差接缝。
    def parse(c):
        return tuple(int(c[i:i + 2], 16) for i in (1, 3, 5))
    glow = parse(BG_GLOW)
    draw.ellipse([-320, -420, 520, 340], fill=glow)
    draw.ellipse([w - 520, -420, w + 320, 340], fill=glow)

    # 底带（展开区）+ 顶边一条极细分隔线。
    band_top = int(h * 0.75)
    draw.rectangle([0, band_top, w, h], fill=BG_BAND)
    draw.rectangle([0, band_top - 2, w, band_top], fill=ICON_STROKE)

    # 品牌图标：直接合成应用的真实 icon（手绘双点母题被用户否掉——安装器上要的是
    # 和桌面/托盘一致的那个 logo）。256px 源图缩到 160，带 alpha 贴上。
    icon_size = 160
    cx = w // 2
    icon = (
        Image.open(OUT_DIR / "128x128@2x.png")
        .convert("RGBA")
        .resize((icon_size, icon_size), Image.LANCZOS)
    )
    img.paste(icon, (cx - icon_size // 2, int(h * 0.20) - icon_size // 2), icon)

    # 字标（语言无关，可烙图）。中心 y=33%。
    font = load_font("segoeuib.ttf", 84)
    draw.text((cx, int(h * 0.33)), "Meowo", fill=TEXT_COLOR, font=font, anchor="mm")

    # 两个变体：主页/完成页用带胶囊按钮的（原生按钮白底在暗色下扎眼，NSIS 裸环境
    # 又做不了 owner-draw，圆角胶囊烙图、安装器叠一条同色可点文字带）；
    # 进度页用不带胶囊的（那页没有按钮，绿胶囊悬在进度条后面很怪）。
    # 胶囊几何必须与 installer.nsi 的按钮文字带对齐：320x52 逻辑 @2x = 640x104，
    # 圆角 24（=逻辑 12，文字带全宽落在直边区内，不会探出圆角）。
    img.save(OUT_DIR / "nsis-oneclick-bg.bmp", "BMP")
    bw, bh = 640, 104
    bx, by = (w - bw) // 2, int(h * 0.56)
    draw.rounded_rectangle([bx, by, bx + bw, by + bh], radius=24, fill=DOT_GREEN)
    img.save(OUT_DIR / "nsis-oneclick-bg-btn.bmp", "BMP")


if __name__ == "__main__":
    make_header()
    make_sidebar()
    make_oneclick_bg()
    print("NSIS bitmaps updated.")
