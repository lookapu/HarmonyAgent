# 工具参数校验与纠错

HarmonyAgent 在原生工具的统一执行入口对参数进行 schema 级预检。校验失败时工具不会执行，错误会作为普通工具失败反馈给 Agent，使其能够修正参数并在下一轮重试。

当前检查范围：

- 参数必须是合法 JSON 对象；
- 注册表声明为必填的字段不可缺失；
- 未在工具 schema 中声明的字段会被拒绝；
- 拼写接近的未知字段会给出候选字段名，但不会直接改写调用；
- token、secret、password、证书、签名材料、profile、keystore、设备序列号等敏感参数会标记为“禁止自动修正”，且建议中不回显字段值；
- MCP 工具由运行期 MCP 服务提供动态 schema，继续由服务端校验。

参数声明同时对原生 function calling 输出 `required` 与 `additionalProperties: false`，让模型在调用前和执行前使用同一份约束。
