import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  // 产物/参考/内置运行时目录不参与 lint
  globalIgnores(['dist', 'portable-build', 'references', 'src-tauri/target', 'src-tauri/runtime']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      // react-hooks 7.x 把 React Compiler 配套规则纳入 recommended：这些规则假设代码经 Compiler 转换，
      // 对未接入 Compiler 的项目会把标准异步加载/effect 内同步 setState 等惯用模式判为错误，
      // 属系统性误报面，予以关闭（保留 rules-of-hooks / exhaustive-deps 经典规则）。
      'react-hooks/set-state-in-effect': 'off',
      'react-hooks/purity': 'off',
      'react-hooks/refs': 'off',
      'react-hooks/preserve-manual-memoization': 'off',
      'react-hooks/immutability': 'off',
      // fast refresh 仅影响开发热更新体验，本文件大量导出常量+组件（图标映射/正则工具等），
      // 不满足 only-export-components 属既有设计，关闭避免噪音。
      'react-refresh/only-export-components': 'off',
    },
  },
])
