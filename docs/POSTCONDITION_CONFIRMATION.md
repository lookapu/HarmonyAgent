# 副作用写后读确认

HarmonyAgent 将工具返回成功与外部真实状态确认分开处理。下列写入在成功后会形成待确认项：

| 写入动作 | 后续确认工具 |
| --- | --- |
| `deploy` / `deploy_all` / 安装 | `get_app_info`、`verify_ui`、`take_screenshot` 或 `read_runtime_logs` |
| `start_ability` | `get_app_info`、`verify_ui` 或 `read_runtime_logs` |
| `git_commit` | `git_status` 或 `git_log` |
| `git_push` | `git_status` 或 `git_log` |
| `git_pull` / merge / rebase | `git_status` 或 `git_log` |
| `db_migrate` | `db_query` |
| `secret_store` | `secret_get` |
| 知识/记忆写入 | `search_knowledge` |
| HTTP POST/PUT/PATCH/DELETE 等 | 独立的 HTTP GET/HEAD/OPTIONS 读取 |

确认必须发生在对应写入之后。最新一次同类写入尚未读取确认时，不能沿用更早一次的确认。部署、Git 提交和 Git 推送的目标契约验收会同时绑定写入证据与后续读取证据；缺任一项均进入补救轮。
