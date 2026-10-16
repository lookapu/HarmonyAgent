#!/usr/bin/env python3
"""官网边界门禁（第十六批 P2-5 配套）。

`website/` 是**唯一一个不受任何自动化检查覆盖**的产物：
已有的 11 道门禁全部只扫 `src/` 与 `docs/`，链接失效、图片 404、
导航锚点指空、有人不小心引入 CDN——这些都不会让任何检查报红。

而它偏偏是**用户第一眼看到的东西**。这两件事放在一起就是一张欠账。

## 契约（10 条）

1. **扫描范围不得为空**：`index.html` 必须含 `data-fill` 槽位。
   入口被换掉或结构大改时，「全部通过」和「没东西可查」在退出码上长得一模一样。
2. **本地资源目标必须存在**：`src` / `href` 的相对路径必须能在磁盘上找到。
   改名图片后页面一片裂图，没有任何检查会报红。
3. **页内锚点必须指向存在的 id**：`href="#x"` 要求文档里真有 `id="x"`。
   删掉一个 section 后导航链接还指着它——点了没反应。
4. **id 必须唯一**：重复 id 让锚点只跳到第一个。
5. **资源不得走外链**：`script[src]` / `link[href]` / `img[src]` 禁止 `http(s)://`
   与协议相对 `//`。这是**离线可用**那条产品口径的落点：官网要在内网机器上能打开，
   CDN 一挂就白屏，而白屏比丑糟得多。
   **`<a href>` 不受此限**——外链（如仓库地址）本来就是网页该做的事，
   把它们一并禁掉只会逼着人删链接。
6. **CSS 不得 `@import` 外链**：契约 5 的 CSS 版本，同样是白屏来源。
7. **`data-fill` 键必须唯一**：重复键让「按 `data-fill` 批量填内容」静默失效
   ——填了其中一个，另一个还是占位，而填内容的人不会发现。
8. **带 `data-fill` 的叶子元素必须有文本**：
   **只判叶子**。容器元素（`<a class="dl" data-fill="download.windows">` 里面
   套 `<strong>` / `<span>`）的 `data-fill` 指的是「这一块」而不是「这段文字」，
   对容器查文本必然误报——这是本门禁第一版真实踩到的坑。
9. **`href="#"` 必须带 `data-fill`**：空锚点是「待填」还是「漏配」，
   只有这一个区分方式。漏配的 `#` 点了会跳到页顶，看起来像坏掉了。
10. **Tailwind 隔离行必须存在**：`src/index.css` 里的 `@source not "../website"`。
    删掉它不会报红，但官网 HTML 的 class 名会被扫进桌面端产物
    （实测：网站里出现 1 个 Tailwind utility 类，桌面端 CSS 就多 1 条无用规则）。
    这一条保证护栏**不被删**——注意它当前是**预防性**的，官网用的全是自定义类名，
    Tailwind 本来就不会为它们生成规则。

11. **语义可访问性（5 条，只管语义存在性）**：
    - `<html>` 必须有 `lang`：读屏靠它选发音引擎，缺了中文页会按英文念
    - **必须恰好一个 `<h1>`**：多个会让读屏的标题导航出现两个入口，零个则整页没有入口
    - **标题层级不得跳级**（h1 → h3）：读屏用户按 h 键逐级跳转，跳级意味着
      「跳到下一个标题」会漏掉一整层结构
    - **纯图标链接/按钮必须有可访问名**：`<a>` / `<button>` 里只有 `<svg>` / `<img>`
      而无文字时，名字只能来自 `aria-label`；`<img>` 的非空 `alt` 也算（可访问名会取到它）
    - **`<a>` / `<button>` 不得完全为空**：既无文字、无 `aria-label`、也无 `title` 时，
      它在读屏里就是一个没有名字的控件，Tab 键能选中但用户不知道按下去会发生什么

    > **为什么官网 a11y 放在本门而不是 `check-a11y.py`**：
    > 后者的判据是 React 专用正则（`<Icon\b`、跨文件 role 统计），
    > 而判 HTML 需要一个真正的解析器。`check-website.py` 已经有一个
    > （`html.parser.HTMLParser`），再写第二个就是**第二份 HTML 解析实现**——
    > 两份实现必然在某次「只改一份」之后开始分歧，而分歧的那一半没有任何检查会发现。
    > 两个门都接在 CI 里，覆盖面是连起来的。

    本门**不管对比度与焦点顺序**——那需要真实浏览器 + axe，
    `check-a11y.py` 的契约 5 也写了同样的话。

12. **对比度（纯 Python 算 WCAG，第十九批新增）**：把 `:root` 的色板变量解出来，
    对**语义上真实存在的配对**算相对亮度比，低于 4.5:1 报红。
    **深色与亮色两套色板都要判**——只看默认值的话，「亮色模式下对比度塌掉」永远测不到。

    > **为什么不引 axe-core**：它是 **MPL-2.0**（文件级 copyleft），
    > 不在许可门禁允许的 BSD-3 / MIT / Apache-2 白名单里，`check-licenses.py` 会挡。
    > 而对比度在静态站上是**纯算术**——给两个色，算亮度比。
    > 为了一个开方运算引入一个文件级 copyleft 依赖，代价与收益完全不成比例。
    > axe 能做而这里做不了的是「真实渲染后的视觉回归」，那是浏览器的事。

    > **「承载白字的面」与「画在底上的字」必须是两个 token**。
    > 本门一上线就在真实官网上抓到：`--site-accent-fg`(白) 压 `--site-accent`(#2f81f7)
    > 只有 **3.75:1**；而把 #2f81f7 压到配得上白字时它对深底又掉到 4.06:1，链接跟着不达标。
    > 合成一个 token 时，**两个角色必然有一个不合规**。

13. **焦点必须看得见（第十九批新增）**：必须存在一条**全局的**（选择器就是
    `:focus-visible`）且产生**可见指示器**（`outline` / `box-shadow` / `border` 上的可见变化）
    的规则。限定选择器（`.site-nav a:focus-visible`）不算——它只覆盖一部分元素。

    > 反过来**不要求每一条** focus-visible 规则都画指示器：
    > `a:hover, a:focus-visible { color: … }` 只是在全局指示器之上加一层配色变化，
    > 是合法增强。把它判成红只会逼人删掉 hover 规则——**判据比问题本身更糟**。

14. **焦点顺序必须跟 DOM 一致（第十九批新增）**：不得出现**正数** `tabindex`
    （`tabindex="0"` 与 `tabindex="-1"` 都合法）。正数会把元素提到所有
    `tabindex=0` 的元素之前，键盘走的路线从此与眼睛看到的顺序对不上。

## 判据的已知边界

本门禁扫的是字面量，它挡不住：

- 图片路径写对了但**指向 404**（外链已被契约 5 挡住；本地文件只看存不存在，
  不校验内容是不是图片）
- `data-fill` 键存在但内容填得**语义不对**（把「特性三」填成 FAQ 的文案）
- 官网的**视觉**问题（间距、排版、动画观感）——对比度与焦点已由契约 12~14 覆盖，
  其余仍属人工评审
- **真实渲染后的视觉回归**：契约 12 算的是「设计稿上声明的色值」，不是浏览器算出来的
  最终像素。渐变、透明度叠加、图片上的文字都测不到——那需要 axe 或人工看

## 用法

    python scripts/check-website.py             # 全仓检查（CI 门禁）
    python scripts/check-website.py --report    # 列出所有待填槽位与占位状态
    python scripts/check-website.py --self-test
"""
from __future__ import annotations

import argparse
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SITE = REPO / "website"
ENTRY = SITE / "index.html"
STYLES = SITE / "assets" / "styles.css"
APP_CSS = REPO / "src" / "index.css"
TAILWIND_EXCLUDE = '@source not "../website";'

#: 允许走外链的属性：只有超链接。外链是网页的本职，禁掉它只会逼人删链接。
EXTERNAL_OK = {("a", "href")}
RESOURCE_ATTRS = ("src", "href")

EXTERNAL_RE = re.compile(r"^(?:[a-z][a-z0-9+.-]*:)?//|^https?:", re.IGNORECASE)
CSS_IMPORT_RE = re.compile(r"@import\s+(?:url\()?['\"]?([^'\")\s;]+)", re.IGNORECASE)
#: 官网一旦出现这些，说明它开始有自己的构建流程，
#: 桌面端 index chunk 只剩 3.3KB 的余量（§6.25.9）就会被牵动
BUILD_EXTS = {".ts", ".tsx", ".jsx"}
BUILD_FILES = {"package.json", "vite.config.ts", "vite.config.js", "webpack.config.js"}
VOID = {"img", "br", "hr", "meta", "link", "input", "source", "area", "base", "col"}
#: 叶子元素的内容也可能待在这些属性里（void 元素没有文本节点）
CARRY_ATTRS = ("content", "src", "alt", "value", "title", "href")


class Slot:
    """一个 data-fill 槽位的解析结果。"""

    __slots__ = ("key", "tag", "line", "text", "is_leaf", "attrs")

    def __init__(self, key: str, tag: str, line: int, attrs: dict[str, str]):
        self.key = key
        self.tag = tag
        self.line = line
        self.text = ""
        self.is_leaf = True  # 见到子元素就翻 False（契约 8 只对叶子判）
        self.attrs = attrs


class Frame:
    """开着的标签。用来判「这个 a/button 里面到底有什么」。"""

    __slots__ = ("tag", "line", "slot", "text", "child_tags", "icon_alt", "attrs")

    def __init__(self, tag: str, line: int, slot: Slot | None, attrs: dict[str, str]):
        self.tag = tag
        self.line = line
        self.slot = slot
        self.attrs = attrs
        self.text = ""
        self.child_tags: list[str] = []
        self.icon_alt = ""  # 子 <img> 的非空 alt，可访问名会取到它

    def accessible_name(self) -> str:
        """可访问名：内部文字 → aria-label → title → 子 img 的 alt。

        顺序按无障碍名计算算法的简化版：**内容优先于属性**。
        """
        return (
            self.text.strip()
            or self.attrs.get("aria-label", "").strip()
            or self.attrs.get("title", "").strip()
            or self.icon_alt
        )


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.ids: list[tuple[str, int]] = []
        self.slots: list[Slot] = []
        self.refs: list[tuple[str, str, str, int]] = []  # (tag, attr, value, line)
        self.images_without_alt: list[int] = []
        self.html_lang: str | None = None
        self.headings: list[tuple[int, int]] = []  # (level, line)
        self.interactive: list[Frame] = []  # a / button 的内部构成
        self._open: list[Frame] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attr = {k.lower(): (v or "") for k, v in attrs}
        line = self.getpos()[0]
        slot: Slot | None = None

        if tag == "html":
            self.html_lang = attr.get("lang", "").strip() or None
        if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.headings.append((int(tag[1]), line))
        if "id" in attr:
            self.ids.append((attr["id"], line))
        if "data-fill" in attr:
            slot = Slot(attr["data-fill"], tag, line, attr)
            self.slots.append(slot)

        # 父槽位见到**任何**子元素就变成容器——不限于子元素自己也带 data-fill。
        # 条件写成「子元素也有槽位」时，`<a data-fill=x><strong>文本</strong></a>`
        # 里的父槽位仍被当成叶子，契约 8 就会对着容器的空文本误报。
        # void 元素（`<img>`）同样算「有内容」，否则 <figure><img></figure> 会被误判成空容器。
        if self._open:
            parent = self._open[-1]
            if parent.slot is not None:
                parent.slot.is_leaf = False
            parent.child_tags.append(tag)
            if tag == "img" and attr.get("alt", "").strip():
                parent.icon_alt = attr["alt"].strip()

        for name in RESOURCE_ATTRS:
            if attr.get(name):
                self.refs.append((tag, name, attr[name], line))
        if tag == "img" and "alt" not in attr:
            self.images_without_alt.append(line)

        if tag not in VOID:
            frame = Frame(tag, line, slot, attr)
            self._open.append(frame)
            if tag in ("a", "button"):
                self.interactive.append(frame)

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        before = len(self._open)
        self.handle_starttag(tag, attrs)
        if len(self._open) > before:
            self._open.pop()

    def handle_data(self, data: str) -> None:
        if not data.strip() or not self._open:
            return
        frame = self._open[-1]
        if frame.slot is not None and frame.slot.is_leaf:
            frame.slot.text += data.strip()
        # 文本记到**所有**开着的 frame 上：外层 a 的可访问名也来自内部文字，
        # 只记栈顶的话 `<a><strong>Windows</strong></a>` 会被判成「无文字」
        for f in self._open:
            if f.tag in ("a", "button") or (f.slot is not None and f.slot.is_leaf):
                f.text += data.strip()

    def handle_endtag(self, tag: str) -> None:
        for i in range(len(self._open) - 1, -1, -1):
            if self._open[i].tag == tag:
                del self._open[i:]
                return

    def find(self, key: str) -> Slot | None:
        return next((s for s in self.slots if s.key == key), None)


PLACEHOLDER = "（待填写"


def is_placeholder(s: Slot) -> bool:
    """该槽位是否仍是占位符。

    判的是**内容本身**，不是展示串。
    第一版拿 `self_text()` 的返回值去 `startswith("（待填写")`，
    而属性槽位的展示串是 `content="（待填写…"` —— 开头是 `content=`，
    于是所有属性槽位都被算成「已填」，占位计数少算了 3 个且**不报红**。
    """
    if s.text:
        return s.text.startswith(PLACEHOLDER)
    if not s.is_leaf:
        return False  # 容器由子槽位各自判定
    for a in CARRY_ATTRS:
        value = s.attrs.get(a, "").strip()
        if value:
            return value.startswith(PLACEHOLDER)
    return True


def check_site(
    html: str,
    css: str,
    app_css: str,
    exists=None,
) -> list[str]:
    """exists(rel_path) -> bool：文件存在判定**可注入**。

    可注入不是为了好看：自测的合成 HTML 引用的是假路径，
    若判据直连真实磁盘，自测就只能验「假阳性」，
    验不了「注入一个真缺陷 → 必须报红」——那等于没做负向验证。
    """
    if exists is None:
        base = ENTRY.parent
        root = REPO.resolve()

        def exists(rel: str) -> bool:
            target = (base / rel).resolve()
            try:
                target.relative_to(root)
            except ValueError:
                return False  # 逃出仓库：判为不存在，交由调用方单独报「逃出仓库」
            return target.exists()

    problems: list[str] = []
    parser = PageParser()
    parser.feed(html)

    # 契约 1：判定面本身不能消失
    if not html.strip():
        return ["扫描范围失效：index.html 是空的——「全部通过」不等于「检查到了东西」"]
    if not parser.slots:
        problems.append(
            "扫描范围失效：index.html 里没有任何 data-fill 槽位——"
            "入口被换掉或结构大改时，本门禁会对着空输入报绿"
        )

    # 契约 4：id 唯一
    id_seen: dict[str, int] = {}
    for value, line in parser.ids:
        if value in id_seen:
            problems.append(f"重复的 id=\"{value}\"（第 {id_seen[value]} 行与第 {line} 行）")
        else:
            id_seen[value] = line
    id_set = set(id_seen)

    # 契约 2 / 3 / 5 / 9：逐个属性判
    for tag, attr, value, line in parser.refs:
        if EXTERNAL_RE.match(value):
            if (tag, attr) in EXTERNAL_OK:
                continue
            problems.append(f"第 {line} 行 <{tag} {attr}> 引用了外链资源 {value}：离线/内网打开会白屏")
            continue
        if value.startswith("#"):
            anchor = value[1:]
            if not anchor:
                if tag == "a" and not _fill_at(parser, line):
                    problems.append(
                        f"第 {line} 行 <a href=\"#\"> 漏了 data-fill："
                        "空锚点是「待填」还是「漏配」只有这一个区分方式"
                    )
                continue
            if anchor not in id_set:
                problems.append(f"第 {line} 行 {value} 指向不存在的 id——点了没反应")
            continue
        if value.startswith(("mailto:", "tel:", "data:")):
            continue
        rel = value.split("?")[0].split("#")[0]
        if rel and not exists(rel):
            problems.append(f"第 {line} 行 <{tag} {attr}> 指向不存在的文件 {value}")

    # 契约 6：CSS 不得 @import 外链
    for m in CSS_IMPORT_RE.finditer(css):
        url = m.group(1)
        if EXTERNAL_RE.match(url):
            problems.append(f"styles.css @import 了外链 {url}：与契约 5 同一个白屏来源")

    # 契约 7：data-fill 键唯一
    fill_seen: dict[str, int] = {}
    for slot in parser.slots:
        if slot.key in fill_seen:
            problems.append(
                f"data-fill=\"{slot.key}\" 重复（第 {fill_seen[slot.key]} 行与第 {slot.line} 行）："
                "按这个键批量填内容时会只填中一个，另一个仍是占位"
            )
        else:
            fill_seen[slot.key] = slot.line

    # 契约 8：**只对叶子**判「必须有内容」
    for slot in parser.slots:
        if not slot.is_leaf:
            continue
        if slot.text.strip():
            continue
        # 属性槽位的内容在属性里而不是文本里：<img alt> / <meta content> 都是 void 元素，
        # 对它们查文本必然误报——这是本门禁第二版真实踩到的坑。
        if any(slot.attrs.get(a, "").strip() for a in CARRY_ATTRS):
            continue
        problems.append(f"第 {slot.line} 行 data-fill=\"{slot.key}\" 的元素既没有文本也没有内容属性")

    # img 必须有 alt
    for line in parser.images_without_alt:
        problems.append(f"第 {line} 行 <img> 缺 alt 属性")

    # ---------- 契约 11：语义可访问性 ----------
    if parser.html_lang is None:
        problems.append(
            "<html> 缺 lang 属性：读屏靠它选发音引擎，缺了中文页会按英文念"
        )

    h1s = [line for lvl, line in parser.headings if lvl == 1]
    if len(h1s) == 0:
        problems.append("整页没有 <h1>：读屏的标题导航会没有入口")
    elif len(h1s) > 1:
        problems.append(
            f"有 {len(h1s)} 个 <h1>（第 {', '.join(str(x) for x in h1s)} 行）："
            "读屏的标题导航会出现多个入口"
        )

    # 层级跳级：h1 → h3 会让「跳到下一个标题」漏掉一整层
    prev_level: int | None = None
    for level, line in parser.headings:
        if prev_level is not None and level > prev_level + 1:
            problems.append(
                f"第 {line} 行标题从 h{prev_level} 跳到 h{level}："
                "读屏按 h 键逐级跳转，跳级会漏掉一整层结构"
            )
        prev_level = level

    for frame in parser.interactive:
        if not frame.accessible_name():
            children = "/".join(dict.fromkeys(frame.child_tags)) or "空"
            problems.append(
                f"第 {frame.line} 行 <{frame.tag}> 没有任何可访问名"
                f"（内容：{children}）——读屏用户 Tab 能选中它，却不知道它是什么"
            )

    # 契约 10：Tailwind 隔离行
    if TAILWIND_EXCLUDE not in app_css:
        problems.append(
            f"src/index.css 里没有 `{TAILWIND_EXCLUDE}`："
            "官网的 class 名会被 Tailwind 扫进桌面端产物"
        )

    problems.extend(check_contrast(css, html))
    problems.extend(check_focus(css, html))

    return problems


# ==================== 契约 12：对比度（纯 Python 算 WCAG，不引 axe） ====================

#: 为什么自己算而不引 axe-core：**axe-core 是 MPL-2.0**，
#: 不在许可门禁允许的 BSD-3 / MIT / Apache-2 白名单里（`check-licenses.py` 会挡）。
#: 而对比度这件事在静态站上是**纯算术**：给出前景/背景两个色，算相对亮度比。
#: 为了一个开方运算引入一个文件级 copyleft 依赖，代价与收益完全不成比例。
#: axe 能做而这里做不了的是「真实渲染后的视觉回归」——那是浏览器的事，不是本门的职责。

#: 正文对比度下限（WCAG 2.1 AA）。3:1 那档只用于 ≥24px 或 ≥19px 粗体的大字，
#: 本门禁**只判正文档**：大字放宽会让「反正大字可以低一点」变成通用借口。
MIN_CONTRAST = 4.5

#: 语义配对：官网承诺的「谁画在谁上面」。
#: 只判**实际被声明过**的配对，不判所有排列——把 --site-fg 配 --site-accent 去算
#: 是无意义的（那两个从不同时出现），而「穷举所有变量两两组合」会产出几十条
#: 永远不会被判红的红。
CONTRAST_PAIRS = [
    ("--site-fg", "--site-bg", "正文 / 页面底色"),
    ("--site-fg", "--site-bg-alt", "正文 / 卡片底色"),
    ("--site-fg-muted", "--site-bg", "次要文字 / 页面底色"),
    ("--site-fg-muted", "--site-bg-alt", "次要文字 / 卡片底色"),
    ("--site-accent", "--site-bg", "链接与强调色 / 页面底色"),
    # 「承载白字的面」与「画在底上的字」是**两个角色**，必须分开判：
    # #2f81f7 配白字只有 3.75:1（不达标），但压到配得上白字时它对深底又只剩 4.06:1，
    # 链接跟着不达标。合成一个 token 时**两个角色必然有一个不合规**——
    # 这是第十九批在真实官网上实测到的，不是推演出来的。
    ("--site-accent-fg", "--site-accent-solid", "按钮文字 / 按钮底色"),
    # --site-line **不在这里**：分隔线不是文字，适用非文本门槛而不是 4.5:1。
    # 第一版把它混进正文配对，于是报「分隔线对比度 1.36:1 不足 4.5:1」——
    # 把一条**判据本身的错**报成了站点的问题。下面 `check_contrast` 末尾
    # 用 1.3:1 单独判它（肉眼可见即可）。
]


def _hex_to_rgb(value: str) -> tuple[int, int, int] | None:
    v = value.strip().lstrip("#")
    if len(v) == 3:
        v = "".join(c * 2 for c in v)
    if len(v) != 6 or not re.fullmatch(r"[0-9a-fA-F]{6}", v):
        return None
    return int(v[0:2], 16), int(v[2:4], 16), int(v[4:6], 16)


def _relative_luminance(rgb: tuple[int, int, int]) -> float:
    def chan(c: int) -> float:
        s = c / 255.0
        return s / 12.92 if s <= 0.04045 else ((s + 0.055) / 1.055) ** 2.4

    r, g, b = (chan(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(fg: str, bg: str) -> float | None:
    a, b = _hex_to_rgb(fg), _hex_to_rgb(bg)
    if a is None or b is None:
        return None
    la, lb = _relative_luminance(a), _relative_luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def _strip_at_media(css: str) -> str:
    """把 `@media ... { ... }` 整块挖掉（自己扫括号，不数正则括号）。

    **必须挖掉**：`:root` 的正则会同时匹配到 `@media (prefers-color-scheme: light)`
    里的那个 `:root`，而它出现在文件后面——于是「默认色板」取到的其实是**亮色**的值。
    第一版就这样把深色模式整个漏检了，而自测因为「亮色也合格」照样全绿。
    """
    out: list[str] = []
    i = 0
    while i < len(css):
        m = re.compile(r"@media[^{]*\{").search(css, i)
        if not m:
            out.append(css[i:])
            break
        out.append(css[i:m.start()])
        depth = 0
        j = m.end() - 1
        for j in range(m.end() - 1, len(css)):
            if css[j] == "{":
                depth += 1
            elif css[j] == "}":
                depth -= 1
                if depth == 0:
                    break
        i = j + 1
    return "".join(out)


def _css_variables(css: str) -> tuple[dict[str, str], dict[str, str]]:
    """取出两套色板：`:root` 的默认值 + `prefers-color-scheme: light` 的覆盖。

    只取**具名色板变量**（值是 #hex），不含 --site-radius / --site-wrap 这类尺寸变量。
    亮色覆盖单独返回是因为**它必须被单独判**——只看默认值的话，
    「亮色模式下对比度塌掉」这条永远测不到。
    """
    def collect(text: str) -> dict[str, str]:
        out: dict[str, str] = {}
        for m in re.finditer(r":root\s*\{([^}]*)\}", text):
            for vm in re.finditer(r"(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,6})\s*;", m.group(1)):
                out[vm.group(1)] = vm.group(2)
        return out

    dark = collect(_strip_at_media(css))
    light: dict[str, str] = {}
    for m in re.finditer(r"@media[^{]*prefers-color-scheme:\s*light[^{]*\{", css):
        rest = css[m.end():]
        for rm in re.finditer(r":root\s*\{([^}]*)\}", rest):
            for vm in re.finditer(r"(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,6})\s*;", rm.group(1)):
                light[vm.group(1)] = vm.group(2)
    return dark, light


def check_contrast(css: str, html: str) -> list[str]:
    problems: list[str] = []
    if not css.strip():
        return ["扫描范围失效：styles.css 是空的——对比度判据没有输入"]

    dark, light = _css_variables(css)
    if not dark:
        return ["扫描范围失效：styles.css 的 :root 里没有任何 #hex 色板变量"]

    for label, palette in (("默认（深色）", dark), ("亮色模式", {**dark, **light})):
        for fg_var, bg_var, what in CONTRAST_PAIRS:
            fg, bg = palette.get(fg_var), palette.get(bg_var)
            if fg is None or bg is None:
                problems.append(
                    f"{label}：{what} 用到的 {fg_var} / {bg_var} 有一个没有定义——"
                    f"这条配对实际不成立，请从 CONTRAST_PAIRS 里删掉"
                )
                continue
            ratio = contrast_ratio(fg, bg)
            if ratio is None:
                problems.append(f"{label}：{what} 的色值无法解析（{fg} / {bg}）")
                continue
            if ratio < MIN_CONTRAST:
                problems.append(
                    f"{label}对比度不足：{what} {fg_var}({fg}) / {bg_var}({bg}) = "
                    f"{ratio:.2f}:1，低于 AA 的 {MIN_CONTRAST}:1"
                )

    # 分隔线不算文字，但它必须**看得见**：低于 1.3:1 肉眼等于没有。
    # 两套色板**都要判**——只判一套的话，「亮色模式下分隔线没了」永远测不到。
    for label, palette in (("默认（深色）", dark), ("亮色模式", {**dark, **light})):
        line, bg = palette.get("--site-line"), palette.get("--site-bg")
        if line and bg:
            r = contrast_ratio(line, bg)
            if r is not None and r < 1.3:
                problems.append(
                    f"{label}分隔线几乎不可见：--site-line({line}) / --site-bg({bg}) = "
                    f"{r:.2f}:1，低于 1.3:1——区块之间会看不出分界"
                )
    return problems


# ==================== 契约 13 / 14：焦点样式与焦点顺序 ====================

def check_focus(css: str, html: str) -> list[str]:
    """焦点必须**看得见**（13），且顺序必须跟 DOM 一致（14）。

    键盘用户靠 Tab 走完全站。看不见焦点 = 这条路直接断掉；
    焦点被正数 tabindex 打乱 = 走的顺序和眼睛看到的顺序不一致，更难用。
    """
    problems: list[str] = []

    # 捕获组**必须包含** `:focus-visible` 本身。写成 `([^{}]*?):focus-visible` 时，
    # 组里只剩前面的空白，`s.strip()` 永远是空串——全局规则因此被当成「空选择器」，
    # 于是「有全局指示器」被判成没有。
    rules = re.findall(r"([^{}]*?:focus-visible)\s*\{([^}]*)\}", css)
    if not rules:
        problems.append(
            "styles.css 里没有任何 :focus-visible 规则："
            "键盘用户 Tab 到链接/按钮时看不到当前位置，导航直接断掉"
        )
        return problems

    def draws_indicator(body: str) -> bool:
        for prop, val in re.findall(r"([\w-]+)\s*:\s*([^;]+);", body):
            v = val.strip().lower()
            if prop == "outline" and v not in ("", "none", "0"):
                return True
            if prop == "outline-width" and v not in ("", "0", "medium", "thin", "0px"):
                return True
            if prop == "box-shadow" and v not in ("", "none"):
                return True
            if prop in ("outline-color", "border-color") and v not in (
                "", "none", "transparent", "currentcolor"
            ):
                return True
        return False

    # 必须有一条**全局的**（选择器就是 :focus-visible）焦点指示器。
    # 限定选择器（`.site-nav a:focus-visible`）不算——它只覆盖一部分元素，
    # 页脚里没被点到的东西就没有指示器。
    #
    # 反过来**不要求每一条** focus-visible 规则都画指示器：
    # `a:hover, a:focus-visible { color: … }` 只是在全局指示器之上加一层配色变化，
    # 是合法增强。把它判成红只会逼人删掉 hover 规则——**判据比问题本身更糟**。
    global_rules = [(s, b) for s, b in rules if s.strip() in (":focus-visible", "*:focus-visible")]
    if not global_rules:
        seen = []
        for s, _b in rules:
            last = s.strip().splitlines()[-1].strip() if s.strip() else "(空选择器)"
            if last not in seen:
                seen.append(last)
        problems.append(
            f"没有**全局**的 :focus-visible 焦点指示器（现有的是 {'、'.join(seen)}）："
            "限定选择器只覆盖一部分元素，"
            "页脚/正文里没被点到的链接与按钮 Tab 过去时看不到当前位置"
        )
    for selector, body in global_rules:
        if not draws_indicator(body):
            problems.append(
                f"全局 `:focus-visible {{ {body.strip()} }}` 没有产生可见指示器"
                f"（需要 outline / box-shadow / border 上的可见变化）"
            )

    for m in re.finditer(r'tabindex\s*=\s*"(-?\d+)"', html):
        n = int(m.group(1))
        if n > 0:
            line = html[: m.start()].count("\n") + 1
            problems.append(
                f"第 {line} 行 tabindex=\"{n}\" 是**正数**："
                "它把该元素提到所有 tabindex=0 的元素之前，"
                "焦点顺序从此与 DOM 顺序不一致——键盘走的路线和眼睛看的顺序对不上"
            )
    return problems


def _fill_at(parser: PageParser, line: int) -> str | None:
    return next((s.key for s in parser.slots if s.line == line), None)


def check_no_build_step() -> list[str]:
    """官网不得自己长出构建流程。"""
    problems = []
    if not SITE.exists():
        return []
    for path in sorted(SITE.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(REPO).as_posix()
        if path.name in BUILD_FILES:
            problems.append(f"{rel}：官网出现了构建入口，与「无构建步骤」的口径冲突")
        if path.suffix.lower() in BUILD_EXTS:
            problems.append(
                f"{rel}：官网出现前端源码文件；一旦开始编译，"
                "桌面端 index chunk 只剩 3.3KB 的余量会被牵动（§6.25.9）"
            )
    return problems


def parse_entry(html: str) -> PageParser:
    parser = PageParser()
    parser.feed(html)
    return parser


def check_page() -> list[str]:
    if not ENTRY.exists():
        return [
            f"入口文件不存在：{ENTRY.relative_to(REPO)}——"
            "官网被挪走或改名时，门禁必须报红而不是安静通过"
        ]
    html = ENTRY.read_text(encoding="utf-8")
    css = STYLES.read_text(encoding="utf-8") if STYLES.exists() else ""
    app_css = APP_CSS.read_text(encoding="utf-8") if APP_CSS.exists() else ""
    return check_site(html, css, app_css) + check_no_build_step()


def collect_slots() -> list[Slot]:
    if not ENTRY.exists():
        return []
    return parse_entry(ENTRY.read_text(encoding="utf-8")).slots


def report() -> int:
    slots = collect_slots()
    if not slots:
        print("没有任何待填槽位")
        return 1
    pending = [s for s in slots if is_placeholder(s)]
    print(f"待填槽位 {len(slots)} 个，其中仍是占位 {len(pending)} 个\n")
    for s in slots:
        shown = self_text(s)
        mark = "待填" if is_placeholder(s) else "已填"
        print(f"  [{mark}] {s.key:26s} 第 {s.line:>4} 行  {shown[:44]}")
    return 0


def self_text(s: Slot) -> str:
    """报告用：叶子取文本/属性值；容器标注为容器。

    刻意**不做**正则 `>…</` 抓取：`<img>` 这类 void 元素没有闭合标签，
    正则会一路跨到下一个闭合标签，把隔壁元素的内容当成本槽位的内容——
    第一版的 `--report` 就是这么把 `hero.image.src` 显示成了 figcaption 的文案。
    """
    if s.text:
        return s.text
    if not s.is_leaf:
        return "（容器，内容在子元素里）"
    for a in CARRY_ATTRS:
        if s.attrs.get(a, "").strip():
            return f"{a}=\"{s.attrs[a]}\""
    return "（既无文本也无内容属性——契约 8 会拦）"


def run() -> int:
    problems = check_page()
    slots = collect_slots()
    pending = sum(1 for s in slots if is_placeholder(s))
    if problems:
        print(f"官网边界校验失败：{len(problems)} 项")
        for p in problems:
            print(f"- {p}")
        return 1
    print(f"官网边界校验通过：{len(slots)} 个待填槽位（{pending} 个仍为占位，内容待填）")
    return 0


# ============================ 自测 ============================

GOOD_HTML = """<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <title data-fill="meta.title">DevEco Switch</title>
  <link rel="stylesheet" href="assets/styles.css" />
</head>
<body>
  <a class="skip-link" href="#main">跳到主内容</a>
  <header><a class="brand" href="#top" data-fill="nav.brand">DevEco Switch</a></header>
  <main id="main">
    <section class="hero" id="top">
      <h1 data-fill="hero.title">（待填写：一句话说明这是什么）</h1>
      <p data-fill="hero.subtitle">（待填写）</p>
      <a class="btn" href="#download" data-fill="hero.cta">（待填写：主行动）</a>
      <img src="assets/hero.png" alt="（待填写）" width="1102" height="752" />
    </section>
    <section class="section" id="download">
      <a class="dl" href="#" data-fill="download.windows">
        <strong>Windows</strong>
        <span data-fill="download.windows.note">（待填写：格式 / 大小）</span>
      </a>
    </section>
  </main>
  <footer><a href="https://github.com/lookapu/DevEco-Switch" data-fill="footer.repo">仓库</a></footer>
  <script src="assets/site.js" defer></script>
</body>
</html>
"""

#: 自测用的「干净 CSS」。**必须自带一份合法色板和一条全局焦点规则**——
#: 契约 12~14 也在 `check_site` 里，GOOD_CSS 不满足它们的话，
#: 下面每一条「期望无问题」的用例都会挂，而这与它们要验的东西无关。
GOOD_CSS = """body { margin: 0; }
:root {
  --site-bg: #0d1117;
  --site-bg-alt: #131a23;
  --site-fg: #e6edf3;
  --site-fg-muted: #9aa7b4;
  --site-line: #263140;
  --site-accent: #2f81f7;
  --site-accent-fg: #ffffff;
  --site-accent-solid: #2972d9;
}
@media (prefers-color-scheme: light) {
  :root {
    --site-bg: #ffffff;
    --site-bg-alt: #f6f8fa;
    --site-fg: #1f2328;
    --site-fg-muted: #59636e;
    --site-line: #d8dee4;
    --site-accent: #0969da;
    --site-accent-fg: #ffffff;
    --site-accent-solid: #0969da;
  }
}
:focus-visible { outline: 2px solid var(--site-accent); outline-offset: 2px; }
"""
GOOD_APP_CSS = f'@import "tailwindcss";\n@source not "../scripts";\n{TAILWIND_EXCLUDE}\n'
#: 合成站点里「存在」的文件。真实判据直连磁盘，自测必须注入——
#: 否则自测只能验「假阳性」，验不了「注入真缺陷必须报红」。
FAKE_FILES = {"assets/styles.css", "assets/hero.png", "assets/site.js", "assets/i.png"}


def _fake_exists(rel: str) -> bool:
    return rel in FAKE_FILES


def self_test() -> int:
    cases: list[tuple[str, str, str, str, bool, str]] = [
        ("干净站点全绿", GOOD_HTML, GOOD_CSS, GOOD_APP_CSS, False, ""),
        # ---------- 契约 12：对比度 ----------
        (
            "正文对比度不足被拦",
            GOOD_HTML,
            GOOD_CSS.replace("--site-fg-muted: #9aa7b4;", "--site-fg-muted: #2b3138;"),
            GOOD_APP_CSS,
            True,
            "对比度不足",
        ),
        (
            "亮色模式对比度不足被拦（默认值合格也不行）",
            GOOD_HTML,
            GOOD_CSS.replace("--site-fg: #1f2328;", "--site-fg: #b9c0c8;"),
            GOOD_APP_CSS,
            True,
            "亮色模式对比度不足",
        ),
        (
            "分隔线几乎不可见被拦",
            GOOD_HTML,
            GOOD_CSS.replace("--site-line: #263140;", "--site-line: #121820;"),
            GOOD_APP_CSS,
            True,
            "分隔线几乎不可见",
        ),
        (
            "色板变量缺失被拦（不是静默跳过）",
            GOOD_HTML,
            # 必须删一个**出现在 CONTRAST_PAIRS 里**的变量：
            # 缺失检查只对被声明过的配对生效，删一个不参与配对的变量测不到它。
            GOOD_CSS.replace("  --site-fg-muted: #9aa7b4;\n", ""),
            GOOD_APP_CSS,
            True,
            "没有定义",
        ),
        # ---------- 契约 13：焦点指示器 ----------
        (
            "焦点规则整体消失被拦",
            GOOD_HTML,
            GOOD_CSS.replace(
                ":focus-visible { outline: 2px solid var(--site-accent); outline-offset: 2px; }",
                "",
            ),
            GOOD_APP_CSS,
            True,
            "没有任何 :focus-visible 规则",
        ),
        (
            "只剩限定选择器的焦点规则被拦（页脚没有指示器）",
            GOOD_HTML,
            GOOD_CSS.replace(
                ":focus-visible { outline: 2px solid var(--site-accent); outline-offset: 2px; }",
                ".site-nav a:focus-visible { color: var(--site-fg); }",
            ),
            GOOD_APP_CSS,
            True,
            "全局",
        ),
        (
            "全局焦点规则只改颜色、没有可见指示器被拦",
            GOOD_HTML,
            GOOD_CSS.replace(
                ":focus-visible { outline: 2px solid var(--site-accent); outline-offset: 2px; }",
                ":focus-visible { outline: none; }",
            ),
            GOOD_APP_CSS,
            True,
            "没有产生可见指示器",
        ),
        (
            "hover 配色增强不得误报",
            GOOD_HTML,
            GOOD_CSS + "\n.site-nav a:hover,\n.site-nav a:focus-visible { color: var(--site-fg); }\n",
            GOOD_APP_CSS,
            False,
            "",
        ),
        # ---------- 契约 14：焦点顺序 ----------
        (
            "正数 tabindex 被拦",
            GOOD_HTML.replace('<footer>', '<footer><a href="#a" tabindex="3">跳</a>'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "正数",
        ),
        (
            "tabindex=0 / -1 不得误报",
            GOOD_HTML.replace('<footer>', '<footer><a href="#download" tabindex="0">甲</a>'),
            GOOD_CSS,
            GOOD_APP_CSS,
            False,
            "",
        ),
        (
            "结构整体消失被拦",
            "",
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "扫描范围失效",
        ),
        (
            "data-fill 全部消失被拦",
            re.sub(r'\s*data-fill="[^"]*"', "", GOOD_HTML),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "扫描范围失效",
        ),
        (
            "图片指向不存在的文件被拦",
            GOOD_HTML.replace("assets/hero.png", "assets/nope.png"),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "不存在的文件",
        ),
        (
            "脚本指向不存在的文件被拦",
            GOOD_HTML.replace("assets/site.js", "assets/gone.js"),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "不存在的文件",
        ),
        (
            "锚点指空被拦",
            GOOD_HTML.replace('href="#download"', 'href="#nowhere"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "不存在的 id",
        ),
        (
            "重复 id 被拦",
            GOOD_HTML.replace('id="top"', 'id="download"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "重复的 id",
        ),
        (
            "资源走 CDN 被拦",
            GOOD_HTML.replace('src="assets/site.js"', 'src="https://cdn.example.com/site.js"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "外链资源",
        ),
        (
            "样式表走 CDN 被拦",
            GOOD_HTML.replace('href="assets/styles.css"', 'href="https://fonts.googleapis.com/css2?family=X"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "外链资源",
        ),
        (
            "协议相对外链被拦",
            GOOD_HTML.replace('src="assets/site.js"', 'src="//cdn.example.com/site.js"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "外链资源",
        ),
        (
            "超链接走外链不得误报",
            GOOD_HTML,
            GOOD_CSS,
            GOOD_APP_CSS,
            False,
            "",
        ),
        (
            "CSS @import 外链被拦",
            GOOD_HTML,
            '@import url("https://cdn.example.com/x.css");\n',
            GOOD_APP_CSS,
            True,
            "@import 了外链",
        ),
        (
            "data-fill 键重复被拦",
            GOOD_HTML.replace('data-fill="hero.title"', 'data-fill="hero.subtitle"'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "重复",
        ),
        (
            "叶子元素无内容被拦",
            GOOD_HTML.replace('data-fill="hero.subtitle">（待填写）', 'data-fill="hero.subtitle">'),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "既没有文本也没有内容属性",
        ),
        (
            "容器元素无自身文本不得误报",
            GOOD_HTML,
            GOOD_CSS,
            GOOD_APP_CSS,
            False,
            "",
        ),
        (
            "漏配的 href=# 被拦",
            GOOD_HTML.replace(
                '<a class="dl" href="#" data-fill="download.windows">', '<a class="dl" href="#">'
            ),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "漏了 data-fill",
        ),
        (
            "img 缺 alt 被拦",
            GOOD_HTML.replace('alt="（待填写）" ', ""),
            GOOD_CSS,
            GOOD_APP_CSS,
            True,
            "缺 alt",
        ),
        (
            "Tailwind 隔离行被删被拦",
            GOOD_HTML,
            GOOD_CSS,
            '@import "tailwindcss";\n@source not "../scripts";\n',
            True,
            "Tailwind",
        ),
    ]

    passed = 0
    failures: list[str] = []
    seen: dict[tuple, str] = {}
    for name, html, css, app_css, expect, keyword in cases:
        # 负向用例必须**先断言注入真的生效**。锚点漂移会让 replace 变成空操作，
        # 用例于是「通过」了——但通过的原因是没注入成功，那比没有用例更糟。
        if expect and (html, css, app_css) == (GOOD_HTML, GOOD_CSS, GOOD_APP_CSS):
            failures.append(f"{name}: 注入没生效（输入与干净样例完全相同）")
            continue
        # 「应当全绿」的用例允许与干净样例**不同**——它们是有意构造的变体
        # （hover 配色增强、tabindex=0）。第一版要求它们必须逐字节等于干净样例，
        # 于是这类**回归防护**用例永远写不出来。
        # 要防的是另一种失效：两条用例输入完全相同，后一条的前半段根本没测到东西。
        sig = (html, css, app_css)
        # 干净基线允许被复现（那是一条「仍然全绿」的回归断言），
        # 但**彼此重复**的两条用例等于只测了一次。
        if sig in seen and seen[sig] != "干净站点全绿":
            failures.append(
                f"{name}: 输入与用例「{seen[sig]}」完全相同——"
                f"本条与前一条同进同退，等于只测了一次"
            )
            continue
        seen.setdefault(sig, name)
        problems = check_site(html, css, app_css, exists=_fake_exists)
        if bool(problems) != expect:
            failures.append(f"{name}: 期望{'有问题' if expect else '无问题'} -> {problems}")
            continue
        if expect and keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 报红了但没提到「{keyword}」-> {problems}")
            continue
        passed += 1

    # 「入口文件不存在」这条要过磁盘，顺手验一下真实仓库的 exists 回调
    missing = check_site(GOOD_HTML, GOOD_CSS, GOOD_APP_CSS, exists=lambda _r: False)
    if not any("不存在的文件" in p for p in missing):
        failures.append("文件存在判定注入失效：exists 恒 False 时应报红")
    else:
        passed += 1

    # 第二组：--report 的呈现逻辑。
    # 这一组是被一个真 bug 逼出来的——属性槽位（<meta content> / <img alt>）的
    # 展示串是 `content="（待填写…"` 而不是 `（待填写…`，
    # 于是「按展示串 startswith 判占位」把所有属性槽位都算成了已填。
    # 那个 bug **不报红、计数还少算**，只靠肉眼才能发现，所以必须有用例钉住。
    report_cases = [
        (
            "文本槽位：占位符算待填",
            '<h1 data-fill="a">（待填写：x）</h1>',
            True,
        ),
        (
            "文本槽位：真实文案算已填",
            '<h1 data-fill="a">DevEco Switch</h1>',
            False,
        ),
        (
            "属性槽位：占位符必须算待填（曾误判为已填）",
            '<meta name="description" content="（待填写：一句话描述）" data-fill="a" />',
            True,
        ),
        (
            "属性槽位：真实值算已填",
            '<img src="assets/a.png" alt="截图" data-fill="a" />',
            False,
        ),
        (
            "容器槽位：由子槽位各自判定，不重复计入",
            '<a data-fill="a"><strong>Windows</strong></a>',
            False,
        ),
    ]
    for name, html, expect in report_cases:
        slots = parse_entry(html).slots
        if not slots:
            failures.append(f"{name}: 没解析出槽位")
            continue
        got = is_placeholder(slots[0])
        if got != expect:
            failures.append(f"{name}: 期望{'待填' if expect else '已填'} -> {got}")
            continue
        passed += 1

    total = len(cases) + 1 + len(report_cases)


    # 第三组：契约 11（语义可访问性）。
    # 单独列出来是因为这五条是**最后加的**——没有负向用例时，
    # 它们可能因为「条件写错导致永远不触发」而看起来一直全绿。
    a11y_cases = [
        (
            "html 缺 lang 被拦",
            GOOD_HTML.replace('<html lang="zh-CN">', "<html>"),
            True,
            "缺 lang",
        ),
        (
            "整页没有 h1 被拦",
            GOOD_HTML.replace("<h1 ", "<h2 ").replace("</h1>", "</h2>"),
            True,
            "没有 <h1>",
        ),
        (
            "两个 h1 被拦",
            GOOD_HTML.replace("</main>", "<h1>第二个主标题</h1></main>"),
            True,
            "个 <h1>",
        ),
        (
            "标题跳级 h1→h3 被拦",
            GOOD_HTML.replace("</section>", "</section><h3>跳级了</h3>", 1),
            True,
            "跳到 h3",
        ),
        (
            "纯图标链接无可访问名被拦",
            GOOD_HTML.replace(
                '<a class="skip-link" href="#main">跳到主内容</a>',
                '<a class="skip-link" href="#main"><svg width="16" height="16"></svg></a>',
            ),
            True,
            "没有任何可访问名",
        ),
        (
            "纯图标链接有 aria-label 放行",
            GOOD_HTML.replace(
                '<a class="skip-link" href="#main">跳到主内容</a>',
                '<a class="skip-link" href="#main" aria-label="跳到主内容">'
                '<svg width="16" height="16"></svg></a>',
            ),
            False,
            "",
        ),
        (
            "纯图标链接靠子 img 的 alt 放行",
            GOOD_HTML.replace(
                '<a class="skip-link" href="#main">跳到主内容</a>',
                '<a class="skip-link" href="#main">'
                '<img src="assets/i.png" alt="图标说明" width="16" height="16" /></a>',
            ),
            False,
            "",
        ),
        (
            "完全空的 a 被拦",
            GOOD_HTML.replace(
                '<a class="skip-link" href="#main">跳到主内容</a>',
                '<a class="skip-link" href="#main"></a>',
            ),
            True,
            "没有任何可访问名",
        ),
        (
            "a 里有嵌套元素但有文字放行",
            GOOD_HTML,
            False,
            "",
        ),
    ]
    for name, html, expect, keyword in a11y_cases:
        if (html, GOOD_CSS, GOOD_APP_CSS) == (GOOD_HTML, GOOD_CSS, GOOD_APP_CSS) and expect:
            failures.append(f"{name}: 注入没生效（输入与干净样例完全相同）")
            continue
        problems = check_site(html, GOOD_CSS, GOOD_APP_CSS, exists=_fake_exists)
        if bool(problems) != expect:
            failures.append(f"{name}: 期望{'有问题' if expect else '无问题'} -> {problems}")
            continue
        if expect and keyword and not any(keyword in p for p in problems):
            failures.append(f"{name}: 报红了但没提到「{keyword}」-> {problems}")
            continue
        passed += 1
    total += len(a11y_cases)


    # 护栏：`passed + len(failures)` 必须等于 `total`。
    # 本脚本第一版正是因为把总数多算了一次，**报出「24/25」却一条 FAIL 都没打印**
    # ——退出码是红的，输出里却看不到任何原因，看起来像门禁坏了而不是像账算错了。
    # 计数式判据自己算错时不会报错，所以这里必须让它**自己报错**。
    if passed + len(failures) != total:
        failures.append(
            f"用例账目不平：passed {passed} + failures {len(failures)} != total {total}；"
            "某个分支既没计入 passed 也没计入 failures"
        )

    print(f"自测 {passed}/{total}")
    for f in failures:
        print(f"  [FAIL] {f}")
    return 0 if passed == total and not failures else 1


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:  # Windows 控制台默认 cp936/cp1252，直 print 中文会抛 UnicodeEncodeError
            stream.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass

    ap = argparse.ArgumentParser(description="官网边界门禁")
    ap.add_argument("--self-test", action="store_true", help="合成站点自测")
    ap.add_argument("--report", action="store_true", help="列出所有待填槽位")
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    if args.report:
        return report()
    return run()


if __name__ == "__main__":
    sys.exit(main())
