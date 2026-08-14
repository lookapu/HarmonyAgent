// Vitest 全局 setup：注册 DOM 断言匹配器（toBeInTheDocument 等）。
// @testing-library/react 检测到全局 afterEach（vitest globals: true）时会自动执行 cleanup，
// 无需手动调用；组件在 jsdom 环境中渲染。
import '@testing-library/jest-dom/vitest'
