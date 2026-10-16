/* ============================================================================
   P2-5 官网脚本 —— 只保留「没有它页面就不成立」的那点交互。

   刻意不做：轮播、滚动动画、入场动画、统计脚本、主题切换持久化。
   官网的第一版是给「还没写内容」用的，脚本越少，
   后面填内容时被自己写的脚本绊倒的机会越少。
   ========================================================================== */
(function () {
  "use strict";

  // 页头在滚动后加一条实线：header 本身有底色，但页面顶部时那条边会显得突兀。
  // 用 IntersectionObserver 而不是 scroll 事件，避免每帧都读布局。
  var header = document.querySelector(".site-header");
  if (header && "IntersectionObserver" in window) {
    var sentinel = document.createElement("div");
    sentinel.setAttribute("aria-hidden", "true");
    sentinel.style.cssText = "position:absolute;top:0;height:1px;width:1px;";
    document.body.prepend(sentinel);
    new IntersectionObserver(function (entries) {
      header.classList.toggle("is-scrolled", !entries[0].isIntersecting);
    }).observe(sentinel);
  }

  // 下载区三个链接现在都是 href="#"。文案填好、链接替换之前，
  // 点一下会跳到页顶且没有任何提示——用户会以为页面坏了。
  // 这里给未配置的下载位加 aria-disabled 并拦掉点击，等填好 href 后自然放行。
  document.querySelectorAll("[data-fill^='download.']").forEach(function (el) {
    if (el.getAttribute("href") !== "#") return;
    el.setAttribute("aria-disabled", "true");
    el.addEventListener("click", function (event) {
      event.preventDefault();
    });
  });
})();
