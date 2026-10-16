// Vitest 全局 setup：注册 DOM 断言匹配器（toBeInTheDocument 等）。
// @testing-library/react 检测到全局 afterEach（vitest globals: true）时会自动执行 cleanup，
// 无需手动调用；组件在 jsdom 环境中渲染。
import '@testing-library/jest-dom/vitest'

// jsdom 不实现 matchMedia，而 themeStore 在**模块加载时**就会调它解析 auto 模式
// （apply → resolveTheme），任何 import 到该 store 的用例都会在收集阶段炸掉。
// 这里补一个最小实现：默认深色（matches=false），并保留可写的 matches 供用例切换。
if (!window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    configurable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }),
  })
}
