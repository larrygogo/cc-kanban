// 内置字体集的**唯一**入口(app 主入口与预览/演示页都 import 这里,不再各自散抄):
// Inter 可变做西文(自托管全平台一致);中文走系统字体(Win 微软雅黑 / macOS 苹方,
// 兜底见 styles.css 的 font-family 栈)——此前内置的 Noto Sans SC 子集约 5MB、woff2 几乎
// 不可再压,是安装包最大的单一成分,为跨平台字形一致不值这个价。
// 只引 latin + latin-ext 两个子集:裸 import "@fontsource-variable/inter" 会把
// cyrillic/greek/vietnamese 共 ~85KB 一并打进安装包,而应用只有中/英两种界面语言,
// 那些子集是纯死重量(浏览器按 unicode-range 不会下载,但 woff2 文件全部进产物)。
// latin-ext 保留:西欧人名/地名的变音符号(é/ü/ñ)在 transcript 里常见。
// 该包不提供按子集拆分的 CSS,声明住在本地 inter-subsets.css。
import "./inter-subsets.css";
