# 官网（P2-5）

面向 HarmonyOS / OpenHarmony 开发者的落地页。**当前状态：结构已就位，文案全部留空。**

## 现在能做什么

打开 `index.html` 就是一个完整的单页骨架：页头导航、Hero、特性卡、截图区、
下载区、常见问题、页脚。样式与响应式已经写好，**所有文字都是「（待填写）」占位**。

本地预览（不需要任何构建步骤）：

```bash
# 任选其一，起个静态服务器即可
python -m http.server 8000
# 然后打开 http://localhost:8000/website/
```

直接双击 `index.html` 也能看，只是部分浏览器在 `file://` 下会限制 `loading="lazy"`。

## 内容怎么填

每个待填位置都带 `data-fill="<唯一键>"`，键名分三段 `区段.字段`：

| 区段 | 位置 | 键示例 |
|---|---|---|
| `meta` | `<head>` 里的 title / description | `meta.title` |
| `nav` | 页头与页脚导航 | `nav.features`、`footer.links.docs` |
| `hero` | 首屏标题、副标题、按钮、主图 | `hero.title`、`hero.image.src` |
| `feature` | 6 张特性卡 | `feature.1.title` ~ `feature.6.body` |
| `screenshots` | 截图与图注 | `screenshots.1.caption` |
| `download` | 三个下载位 | `download.windows` |
| `faq` | 4 条常见问题 | `faq.1.q` / `faq.1.a` |

几条约定：

1. **只改文字，不用动结构。** 卡片、FAQ 都是可复制的独立块，增删各留一份即可。
2. **加图**：文件放进 `assets/screenshots/`，然后复制一个 `<figure class="shot">` 改 `src`。
   图片的 `width` / `height` 照着填（现有素材是 1102×752），**留着能避免加载时页面跳动**。
3. **下载链接**：现在是 `href="#"`。替换前 `site.js` 会拦掉点击并置 `aria-disabled`，
   因为占位状态下点下去只会跳到页顶、看起来像坏掉了。换成真实地址后自然放行。
4. **替代文本（`alt`）不要漏。** 截图区的 `alt` 是给读屏用户和搜索用的。

## 为什么不引第三方 CDN

字体、图标、统计脚本全部不引。官网要能在离线机器和内网里打开，
CDN 一挂就白屏，而白屏的官网比丑的官网糟得多。图标用内联 SVG，字体走系统栈。

样式与桌面端是**两套**：这里用 `--site-*` 前缀的变量，不复用 app 的 `--app-*` token。
桌面端的视觉改动不应该顺带改掉官网。

## 与桌面端构建的关系

**这个目录不参与 `npm run build`。** `src/index.css` 里有：

```css
@source not "../website";
```

Tailwind v4 的自动 source 检测**按文件而非按扩展名**扫全仓，
不排掉的话官网 HTML 里的 class 名会被扫进去、把永远不会用到的规则打进桌面端产物。
同一条规则也用于 `scripts/`（理由见 `src/index.css` 顶部注释）。
