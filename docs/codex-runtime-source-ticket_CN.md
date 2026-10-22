# Codex Server v3 来源票据快照与 IPC 原值传输

日期：2026-10-15。

本次只修改 codex-server main。基于本地 `c56e646d0` 的官方 0.162.1
运行时，按用户确认将此前本地 main 的升级和身份修正一起发布。保留
`protocolVersion: 3`、`cpa/inference/body`、官方可执行文件名称和请求身份，
不合并实验 v5 分支，不修改 CPA、CPAMP 或 NewAPI。

## 可选协议扩展

`cpa/capabilities/read` 增加可选 `ticketSnapshots: true`。调用者只有确认
该能力后，才在 `cpa/inference/start.params` 顶层增加
`captureTicketSnapshot: true`。省略或 `false` 时不发送新通知；标志不进入
业务 `request`，也不更改正常对话的票据选择或 Cookie 发送行为。

通知沿用现有受信任连接：

```json
{
  "method": "cpa/inference/ticket",
  "params": {
    "requestId": "本轮请求 ID",
    "credentialId": "account.json",
    "model": "实际请求模型",
    "observedAt": "2026-10-15T03:00:00Z",
    "state": "原始 x-codex-turn-state",
    "route": {
      "__cflb": { "value": "原始值" },
      "__oailb": {
        "value": "原始值",
        "expires_at": "2026-10-15T03:20:00Z"
      }
    }
  }
}
```

`expires_at` 仅在有已知期限时提供，未知则省略，不补造一小时有效期。
`observedAt` 只表示观测时间，不能作为来源票据新的签发时间。接收方仍须
按票据原始签发信息执行来源的一小时期限、提前十分钟续期等既有规则。

响应头快照可能先于 start 接受响应到达；SSE 元数据快照可能在正文中途
到达。快照交付在该轮完成或错误通知之前，不能将它当成业务 SSE。
本次采集覆盖 Responses；独立 Images 路径不新增快照采集。

## 配对与期限

- 每次实际 HTTP 交换单独创建采集状态，包括调用方决定重试后发起的新请求；worker 不会在收到 401 后自动重发推理。
- 请求默认头和原生 Cookie 已应用后，以实际请求中的 Cookie 为候选。
  同次响应 `Set-Cookie` 覆盖对应值；未更新的一枚仅继承该次请求的值。
- 票据只取同次响应头，或明确的 `response.metadata` /
  `codex.response.metadata` SSE 事件中的 headers，不扫描模型文本。
- 两枚 Cookie 和票据不完整、明确删除、已过期时不发可用快照。
  身份归属与模型取自当前 worker 和实际准备好的原生请求。
- `Max-Age`、`Expires`、Cookie 内可解析的 `exp` 保留较早期限。重复观察
  同值不会延长已知期限。Cookie 的 `exp` 只能收紧导出期限，不用来证明身份。
- 原生 Cookie 存储旁保留按名称与值摘要匹配的到期时间下界，用于继承请求
  Cookie 已知期限；不从该记录选择 Cookie，也不改变原生 Jar 的更新策略。
  该记录只会收紧期限，不提供其他交换的 Cookie 值或票据。
- SSE 快照解析器对单个候选元数据设置 64 KiB 的内存上限，超出只跳过
  快照解析，原始业务字节继续完整转发，不新增业务大小限制。

## IPC 脱敏审计与移除

本节按用户最新指令执行：IPC 内容传原值，普通日志单独处理。

| 路径                                            | 审计结果与处理                                                                       |
| ----------------------------------------------- | ------------------------------------------------------------------------------------ |
| `cpa/inference/upstream` 请求 headers           | 移除 Authorization、Cookie、代理认证和 token/API-key 等名称的值替换，按实际值传递。  |
| `cpa/inference/upstream` 响应 headers           | 移除 Set-Cookie 等值替换，turn-state 保持原值，保留多值头。                          |
| start 接受响应 headers                          | 移除因隐私设置的白名单，返回实际响应头；既有来源身份映射继续执行。                   |
| start 错误及 `cpa/inference/error` headers/body | HTTP 错误头改为原值；原有错误正文保持完整。                                          |
| `cpa/inference/ticket`                          | state 与两枚 Cookie 直接序列化原值，不使用替代值或摘要代替。                         |
| master ↔ worker 转发、stdio、WebSocket         | 原有结构化消息原样转发；保留请求归属、凭据检查、RPC ID 映射。                        |
| `cpa/inference/body`                            | 保留原字节流和既有身份映射，不注入快照，不因隐私删除或截断正文。                     |
| `accessTokenSha256`、`oaiLbNode`                | 保留已有兼容字段；它们是额外诊断信息，原始 Authorization/Cookie 同时存在于 headers。 |
| stderr/JSONL/发送失败日志                       | 使用独立副本处理凭据和票据，不记录新快照原值；发送失败日志不格式化整条待发消息。     |

原生请求构造中阻止调用者覆盖 worker 的认证、设备身份等规则属于既有
执行权限约束，不是 IPC 脱敏，本轮保持不变。

## 独立真实 worker 验证

仓库新增本地模拟上游入口，不使用真实凭据、不需要 CPA：

```sh
CODEX_TEST_BINARY=/absolute/path/to/codex-app-server \
  node deploy/cpa-runtime/check-ticket-snapshot.mjs
```

该入口启动真实 master 与两个 worker，运行默认关闭、并发来源、响应头
快照、分块 SSE 元数据和删除 Cookie 场景，核对原始 IPC 头与普通日志。

针对性 Rust 回归包括：Cookie 更新与继承、过期、删除、重复观察不续命、
Cookie `exp`、已知 Jar 期限、SSE 分块、忽略模型文本、并发凭据隔离、
401 原始失败及零 worker 重试下不混票、快照在接受响应前/正文中/结束前交付、原始 IPC 与业务流。
本次最终本地验收结果：

| 检查                               | 结果                                                                        |
| ---------------------------------- | --------------------------------------------------------------------------- |
| 服务单元测试                       | 21 项通过，其中 6 项为新增快照测试。                                        |
| 真实 master/worker 集成测试        | 66 项通过，其中 11 项为新增快照用例。                                       |
| HTTP-client 库                     | 115 项通过，串行执行。                                                      |
| 协议库与生成一致性                 | 320 项通过；默认忽略的生成器测试已在稳定/实验导出生成时分别显式运行。       |
| 严格 Clippy                        | app-server、app-server-protocol、http-client 的库和测试通过 `-D warnings`。 |
| 发布版本准备                       | 5 项通过。                                                                  |
| 最终 `cargo build --locked`        | 通过。                                                                      |
| 最终二进制真实 worker 探针         | 5 次实际请求、3 份预期快照；原始 IPC 头、默认关闭、并发与日志隔离通过。     |
| 最终二进制身份检查                 | `codex-app-server 0.162.1`、`codex-tui/0.162.1` 与原生平台后缀通过。        |
| Rust/文档/示例格式及 diff 空白检查 | 通过。                                                                      |

最终 macOS arm64 联调产物：
`/private/tmp/codex-source-ticket-v3-20261015/codex-app-server`。
SHA256：`bd50bf6ad82e5259c5db73f161d704f6ca6474051c879828ff455624876a84f5`。
测试日志位于 `/private/tmp/codex-ticket-final-service-tests.log`、
`/private/tmp/codex-ticket-http-tests.log`、
`/private/tmp/codex-ticket-protocol-tests.log`、
`/private/tmp/codex-ticket-final-probe.log` 与
`/private/tmp/codex-ticket-final-identity.log`。
这些是本地验证结果，不能代替云端 Linux 镜像发布成功的确认。

## 兼容与部署范围

旧 CPA 不发送采集标志，因此不会收到未知 ticket 通知，正常 v3 推理保留。
IPC 原值传输按用户要求始终生效，包括未开启快照的请求。
新客户端连接旧 server 时，缺少 `ticketSnapshots` 应视为不支持来源快照，
继续原有推理，不发送新参数。

本次没有实现或验收 CPA 侧快照接收、持久化、主动续期、跨实例来源导出与
借用闭环，不能据此宣称实例间借票已端到端可用。main 提交沿用现有镜像
工作流及标签策略，只核对构建启动，不等待构建发布结果。
