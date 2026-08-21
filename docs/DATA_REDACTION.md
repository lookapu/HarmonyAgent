# Agent 数据脱敏边界

所有工具出口（包括 MCP 成功与失败）、工具审计、`ToolResultV2`、可恢复人工交互、会话分享和快照统一使用 `utils::redact`。长输出先脱敏再写入产物，避免完整日志绕过上下文遮罩。

## 默认隐藏

- API key、Bearer、JWT、GitHub/AWS token 和通用 secret/password/credential 字段；
- 私钥块、证书块、keystore/p12 路径与口令、provisioning profile；
- 名称含 token、secret、password、credential 或 private key 的敏感环境变量；
- `device_id`、device serial、serial number 和 UDID；
- 连接 URL 中的明文口令；
- 邮箱、手机号和身份证号（保留少量结构供人工辨认）。

JSON 对象按字段语义递归处理，也识别 `{ "name": "Authorization", "value": "..." }` 结构；字符串叶子继续应用自由文本规则。普通 `PATH`、源码 URL、计数和非敏感配置保持原样，降低误遮蔽。

## 处理顺序

1. 工具成功或失败文本在执行出口脱敏。
2. 超长输出将脱敏后的全文保存为本地产物，再向模型返回有界摘要。
3. 持久化审计若是合法 JSON，优先按字段递归脱敏；否则按自由文本处理。
4. 人工审批/提问的请求与响应、会话导出和快照复用同一 JSON 入口。

回归测试覆盖 token、私钥、证书、签名材料、敏感环境变量、设备标识、连接口令、JSON name/value 结构及普通代码不误伤。
