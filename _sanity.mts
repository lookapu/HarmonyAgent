import { sanitizeToolMarkers } from './src/chat/chatUtils.ts'
const cases = [
  ['前置【TOOL|read_file|{"path":"a"}|结果】后置', '前置后置'],
  ['【TOOL|run_cmd|{"cmd":"ls"}|]}正文', '正文'],
  ['行一\n【TOOL|open_file|{"path":"b"}\n行二', '行一\n\n行二'],
  ['开头【TOOL|edit_file|{"path":"x"}正文泄漏', '开头'],
  ['示例：arr[1] = 2，无工具标记', '示例：arr[1] = 2，无工具标记'],
  ['我来修改文件。\n【TOOL|edit_file|{"path":"a.txt","old":"x","new":"y"}\n】\n修改完成。', '我来修改文件。\n\n修改完成。'],
  ['前置\n【TOOL|edit_file|\n{"path":"a.txt"}\n】\n后置', '前置\n\n后置'],
  ['【TOOL|read_file|{"path":"a"}请看结果', '请看结果'],
  ['开始\n【TOOL|read_file|{"p":"a"}\n】\n中间\n【TOOL|edit_file|{"p":"b","v":"line1\nline2"}\n】\n结束', '开始\n\n中间\n\n结束'],
  ['正文开始\n【TOOL|edit_file|{"path":"x","old":"abc', '正文开始\n'],
  ['配置项 { key: value } 和数组 [1, 2, 3] 正常显示', '配置项 { key: value } 和数组 [1, 2, 3] 正常显示'],
]
let pass = 0, fail = 0
for (const [input, expected] of cases) {
  const got = sanitizeToolMarkers(input)
  const ok = JSON.stringify(got) === JSON.stringify(expected)
  console.log(ok ? 'PASS' : 'FAIL')
  if (!ok) {
    console.log('  input   :', JSON.stringify(input))
    console.log('  expected:', JSON.stringify(expected))
    console.log('  got     :', JSON.stringify(got))
    fail++
  } else pass++
}
console.log(`\n${pass} passed, ${fail} failed`)
process.exit(fail ? 1 : 0)
