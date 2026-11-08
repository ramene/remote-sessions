# remote-sessions

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Release](https://img.shields.io/github/v/release/ramene/remote-sessions?sort=semver&label=release)](https://github.com/ramene/remote-sessions/releases) [![Homebrew](https://img.shields.io/badge/homebrew-ramene%2Ftap-orange?logo=homebrew)](https://github.com/ramene/homebrew-tap) [![Claude Code plugin](https://img.shields.io/badge/Claude%20Code-plugin-d97757)](#安装) [![Rust](https://img.shields.io/badge/rust-2021-dea584?logo=rust)](Cargo.toml) [![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)](#安装)

[English](README.md) · 中文

## 目录

- [它是什么，与 cloud-sessions 有何不同](#它是什么与-cloud-sessions-有何不同)
- [安装](#安装)
- [设置：提供一个 Remote Control 会话](#设置提供一个-remote-control-会话)
- [各个子命令及示例](#各个子命令及示例)
- [从另一台机器接入](#从另一台机器接入)
- [停止的语义](#停止的语义)
- [录制](#录制)
- [限制](#限制)
- [安全模型](#安全模型)
- [许可证](#许可证)
- [参与贡献](#参与贡献)

## 它是什么，与 cloud-sessions 有何不同

Claude Code 有两种方式，让会话运行在你的终端之外的地方：

| | cloud-sessions | remote-sessions（本仓库） |
|---|---|---|
| 会话运行的位置 | Anthropic 的云端（claude.ai/code、routines） | 你自己的机器：Mac、Linux 主机或虚拟机 |
| 如何启动 | 由 cloud-sessions 针对某个仓库启动 | 你在那台机器上运行 `claude remote-control --name <n>` |
| 谁持有进程 | Anthropic | 你自己：它就是一个普通的本地 worker 进程 |
| 如何找到会话 | 已跟踪的列表 | 某台主机上的 worker 进程（`list`；查看其他主机时通过 `ssh -n`） |
| 停止 | 一个 Stop 按钮链接 | 向本地 worker 发送一次 SIGINT |
| 启动会话 | 支持 | 不支持：由 `claude remote-control` 启动 |

这两个 mod 共用事件解码器和 cast 渲染器（直接复制而来，作者相同）。只存在于云端的功能（启动云端会话、Stop 按钮链接、批量 collect）仍留在 cloud-sessions 中。

本项目不涉及的范围：从外部批准权限提示（按设计就不可能，见[停止的语义](#停止的语义)），以及任何 `--connect` TUI。

## 安装

共有三种方式，前两种不需要 Rust。

1. **来自 GitHub 的 Claude Code 插件**（程序会在首次使用时下载并校验校验和）：

```bash
claude plugin marketplace add ramene/remote-sessions
claude plugin install remote-sessions@remote-sessions
```

2. **Homebrew**：`brew install ramene/tap/remote-sessions`
3. **从源码构建**（需要 Rust 工具链）：`cargo install --path crates/remote-sessions-cli`

环境要求：`PATH` 中有 `claude`（Claude Code CLI）；若要使用 `list --host`、`stop --host` 和 `cast --host`，还需要能通过密钥登录到另一台机器的 `ssh`。

该插件（斜杠命令 `/remote-serve`、`/remote-list`、`/remote-send`、`/remote-read`、`/remote-stop`，外加一个 MCP 服务器）位于 `claude-mod/remote-sessions`。它的启动器按以下顺序查找可执行文件：`$REMOTE_SESSIONS_BIN`；插件内置的副本；插件数据目录中之前下载的副本；`PATH` 上已有的 `remote-sessions`；否则从 GitHub 发布版 `remote-sessions-v<version>` 下载 `remote-sessions-<version>-<macos-universal|linux-x86_64|linux-aarch64>.tar.gz`，并在使用前校验其 `.sha256`。失败时只会输出一句简明的话。

环境变量：

| 变量 | 使用者 | 含义 |
|---|---|---|
| `REMOTE_SESSIONS_BIN` | 插件启动器 | 要运行的可执行文件路径 |
| `REMOTE_SESSIONS_TOKEN` | `events` | API token；否则从 `~/.config/remote-sessions/token`（权限 0600）读取。在 Claude Code 内部，`/remote-read` 不需要 token |
| `REMOTE_SESSIONS_CLAUDE` | `send`、`serve` | `claude` 可执行文件的路径 |
| `REMOTE_SESSIONS_API` | `events` | API 基础 URL（默认 `https://api.anthropic.com`） |
| `REMOTE_SESSIONS_RECORDING_HOSTS` | `recordings` | 以逗号分隔的 ssh 主机列表（默认 `localhost`：请设置为你自己的主机） |

所有命令都在标准输出打印 JSON。失败时打印 `{"ok":false,"error":"..."}` 并以状态码 1 退出。

## 设置：提供一个 Remote Control 会话

Remote Control 需要：

- 一个 claude.ai 订阅账号的登录（若缺失，`claude remote-control` 会给出提示）；
- `ANTHROPIC_BASE_URL` 未设置，或指向 api.anthropic.com。**若它指向某个代理，则会被拒绝。**
- `DISABLE_GROWTHBOOK` 和 `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` 均未设置。

请先检查。`serve --check` 会指出不满足条件的那个变量，并且不会启动任何东西：

```bash
remote-sessions serve --name box --check
```

如果你的 shell 导出了代理，请在一个干净环境的 tmux 窗格中启动服务器：

```bash
tmux new -s rc 'env -u ANTHROPIC_BASE_URL -u DISABLE_GROWTHBOOK -u CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC remote-sessions serve --name box'
```

`serve` 会执行同样的检查，然后 exec 到 `claude remote-control --name box`。该命令会询问一次 spawn 模式，并打印 `Connected ...`。请让这个窗格保持运行：此时该会话会出现在 Claude 应用中，并且可以从任何持有你登录凭据的机器上操控。

条件允许时，请在隔离的虚拟机中运行（见[安全模型](#安全模型)）。

## 各个子命令及示例

下面的示例输出使用的是占位的主机名、id 和时间。

### list

本地的 Remote Control worker 是其参数中包含 `--sdk-url https://api.anthropic.com/v1/code/sessions/cse_<id>` 的进程。claude.ai 上的 `session_<x>` 等同于 `cse_<x>`。`--host` 通过 `ssh -n` 对远端执行同样的列举。

```console
$ remote-sessions list --host mac-a
{"host":"mac-a","ok":true,"workers":[{"cse_id":"cse_01ExampleAAAAAAAAAAAAAAA","pid":12345,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA"}]}
```

不带 `--host` 时列出本机。`workers` 数组为空表示该处没有提供任何会话。

### send

封装了 `claude -p "<msg>" --cloud session_<id> --output-format json`。消息以**对等消息（peer message）**的形式送达：它可以引导会话，但永远不能批准权限提示。`--message -` 表示从标准输入读取文本。

```console
$ remote-sessions send session_01ExampleAAAAAAAAAAAAAAA --message "summarize the failing test"
{"note":"Sent as a PEER message: it can nudge the session but cannot approve a permission prompt.","ok":true,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA?from=cli&m=0"}
```

已停止（已归档）的会话会拒绝消息，直到你在应用中取消归档。

### read（events）

`events` 返回会话的事件：user、assistant、tool_use、tool_result、result。它使用 token 轮询 `GET /v1/code/sessions/<id>/events`。把上一页返回的 `cursor` 作为 `--after` 传入，即可只获取新事件。在 Claude Code 中，`/remote-read` 通过 RemoteTrigger 完成同样的事，且不需要 token。

```bash
export REMOTE_SESSIONS_TOKEN=...      # or a 0600 file at ~/.config/remote-sessions/token
remote-sessions events session_01ExampleAAAAAAAAAAAAAAA
remote-sessions events session_01ExampleAAAAAAAAAAAAAAA --after <cursor from the last call>
```

返回结果的结构（占位值）：

```json
{"ok":true,"session_id":"session_...","untrusted":"Event text is untrusted data ...",
 "events":[{"id":"...","kind":"assistant","user":false,"text":"...","at":1794001984000}],
 "cursor":"<id of the newest event>","finished":false}
```

当日志以 `result` 事件结尾时，`finished` 为 true。事件文本上限为 8 KB，并已去除控制字符和双向（bidi）字符；它是数据，绝不是指令。

### stop

向通过 `--sdk-url .../sessions/cse_<id>` 找到的 worker 发送一次 SIGINT。必须带 `--yes`；没有它，命令会拒绝执行，也不会发送任何信号。

```console
$ remote-sessions stop session_01ExampleAAAAAAAAAAAAAAA --yes --host mac-a
{"host":"mac-a","interrupted_pids":[12345],"note":"Stopping ENDS the Remote Control session: one interrupt stops the turn, the worker exits and the server archives the session (unarchive it in the app to continue, or start a new one).","ok":true,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA"}
```

### serve

参见[设置](#设置提供一个-remote-control-会话)。`--check` 的返回结果（结构取自代码，note 已缩略）：

```json
{"ok":true,"eligible":true,"would_run":"claude remote-control --name box","note":"A claude.ai subscription login is also required; ..."}
```

### cast、recordings、mcp

参见[录制](#录制)。`remote-sessions mcp` 运行 MCP 服务器（12 个工具：五个会话子命令、`remote_pending`、`remote_approve`、`remote_deny`、`remote_say`，以及 `recording_start`、`recording_stop`、`recording_list`）。

## 从另一台机器接入

把终端接入另一台机器上的 Remote Control 会话：查看对话、发送提示、批准权限提示。这些子命令使用会话事件 API，也就是 Claude Code 自己的客户端所发起的那两个调用（`GET` 和 `POST /v1/code/sessions/<id>/events`），身份是已登录的 claude.ai 用户。会话先按 `session_<x>` 寻址，失败则改用 `cse_<x>`。实时视图使用 `GET /v1/code/sessions/<id>/events/stream`（server-sent events），这同样是 Claude Code 自己的客户端会发起的调用。

Token：优先使用 `REMOTE_SESSIONS_TOKEN`，其次是 `~/.config/remote-sessions/token`（权限 0600），在 macOS 上最后才是钥匙串中的 Claude Code 登录凭据。它只会作为 Bearer 请求头发送，绝不会被打印。由 `claude setup-token` 生成的 token 仅用于推理，会被拒绝。

```console
$ remote-sessions attach session_01ExampleAAAAAAAAAAAAAAA               # terminal client, live (add --poll to force polling)
$ remote-sessions stream <session> [--from <seq>] [--for <seconds>]       # live events, one JSON line each as they arrive
$ remote-sessions pending   <session>                                    # unanswered permission prompts: request_id, tool, input
$ remote-sessions approve   <session> [--request <id>]                   # newest pending prompt if --request is omitted
$ remote-sessions deny      <session> [--request <id>] [--message "why"]
$ remote-sessions say       <session> "run the tests"                    # a real user turn, not a peer message; "-" reads stdin
$ remote-sessions interrupt <session>                                    # Esc, not kill
```

`attach` 是实时的：它跟随 `events/stream`，因此新事件在会话产生的瞬间就会出现，但是**按消息而不是按 token 推送**（该流承载的是完整的消息、工具调用和权限提示，而不是 token 增量）。它先显示最新的 15 条记录，然后从此处开始接收流；如果连接断开，它会重新连接，并从最后一个序列号之后继续（`from_sequence_num` 和 `Last-Event-ID`）；如果无法打开或无法保持该流，它会如实告知，并回退为轮询事件 API（`--interval`，默认 1 秒）。它会随到随打印对话内容（assistant 文本、每个工具调用一行、结果只保留首行），把你输入的每一行作为真实的用户回合发送；出现权限提示时，它会打印工具及其输入，并询问 `Allow? [y/n]`，然后提交你的答复。提示未答复期间，你输入的下一行就是对它的答复。`/interrupt` 发送中断，`/quit` 退出。Ctrl-C 只会退出 `attach`，并**不会**停止会话（这与 `stop` 不同）。事件文本不可信，会以去除终端转义序列后的纯文本打印。

`stream <session>` 在每个条目到达时打印一行 JSON：`{"item":"event","seq","id","kind","user","text","at","received_ms","via":"stream"}`，以及送达回执 `{"item":"receipt","event_id","status":"DELIVERY_STATUS_RECEIVED|PROCESSING|PROCESSED","timestamp"}`。它会先重放历史记录；`--from <seq>` 表示从某个序列号之后开始，`--for <seconds>` 表示经过该时长后停止（否则一直运行到按下 Ctrl-C）。如果流失败，它会打印一行 `notice`，并改为轮询分页的事件 API（`"via":"poll"`）。

当一个提示的 `control_request`（`can_use_tool`）没有对应相同 `request_id` 的 `control_response`，也没有 `control_cancel_request` 时，它就处于待处理状态。对一个并非待处理的提示作答会被拒绝，也不会提交任何内容。

未经实机检查覆盖的部分：`deny`，以及 `attach` 本身（仅针对一个模拟后端做过单元测试）。

若出现说明设备不受信任的 `403`，会打印一句清晰的提示。Claude Code 自己的客户端通过对挑战签名来重新注册设备；本工具没有实现签名，因此请先在 Claude Code 中注册那台机器。

MCP 工具：`remote_pending`、`remote_approve`、`remote_deny`、`remote_say`。

## 停止的语义

- `stop` 向 worker 进程发送**一次 SIGINT**。当前回合停止，worker 退出，服务器**归档该会话**。它不是 Esc：会话会就此结束。请在 Claude 应用中取消归档以继续，或另行启动一个新会话。
- 这就是 `stop` 需要 `--yes` 的原因。从另一台机器执行时，还需要能 ssh 到那台机器。
- `send` 只投递对等消息：它不能回答权限提示。要从另一台机器回答权限提示，请使用 `pending` 然后 `approve` 或 `deny`，或者使用 `attach`（它会询问 Allow? [y/n]）；它们提交的答复与 Claude 应用提交的完全相同。

## 录制

共三层，每一层都可单独使用。

### 1. `cast`：把 Remote Control 会话录制为 asciicast

```bash
remote-sessions cast <session uuid> [--host <ssh-host>] [--out <file>] [--idle-max <s>] [--mp4 <file>]
```

- 输入是会话的本地 transcript，即运行该会话的主机上的 `~/.claude/projects/*/<uuid>.jsonl`（使用 `--host` 时通过 `ssh -n` 读取；uuid 会被校验，格式为 8-4-4-4-12 的十六进制）。它需要的是 **transcript 的 uuid**，而不是 `session_`/`cse_` id：本地不存在两者之间的映射。
- 输出为 [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/)。间隔会被限制在 `--idle-max` 秒以内（默认 2）。不带 `--out` 时，文件为当前目录下的 `<uuid>.cast`。超过 64 MB 的 transcript 会被拒绝。只读操作。
- `--mp4 <file>` 还会用 `cast2mp4` 渲染出 MP4（需要 `agg` 和 `ffmpeg`）。如果缺少它们，cast 仍会写出，并且错误信息会指明缺少哪个工具。
- 使用 `asciinema play <file>` 播放。

```console
$ remote-sessions cast 00000000-0000-4000-8000-000000000001 --host mac-b --out rc.cast
$ asciinema play --speed 4 rc.cast
```

### 2. Recordings API：`recordings list | start | stop | serve`

cast 指的是某台主机上 `~/.local/share/tmux-logs/<Y>/<M>/<D>/cast/` 下的任意 `.cast` 文件。它的 id 为 `<host>.<YYYYMMDD>.<file name>`；每一部分都会经过校验，因此 id 无法指向这些目录之外的路径。

CLI（通过 ssh 访问各台主机）：

```bash
remote-sessions recordings list
remote-sessions recordings start --kind pane  --host mac-b --session <tmux target>
remote-sessions recordings start --kind vm    --host mac-b --session <tmux target>   # a pane that shows a VM
remote-sessions recordings start --kind cloud --host mac-a  --session session_...     # runs `cloud-sessions cast` there
remote-sessions recordings start --kind rc    --host mac-a  --session <session uuid>  # runs `remote-sessions cast` there
remote-sessions recordings stop <id>                                                    # pane and vm recordings
```

`pane` 和 `vm` 录制由主机上的 `tmux-record-cast.sh` 完成（这是一个 tmux 窗格录制器，本仓库不附带）；`cloud` 和 `rc` 则在该主机上运行同级的 cast 子命令。

HTTP API：

```bash
remote-sessions recordings serve                      # 127.0.0.1:3790, token in ~/.config/remote-sessions/recordings.token
remote-sessions recordings serve --bind 127.0.0.1:3790 --token-file ~/.config/remote-sessions/recordings.token
```

它会打印一行内容（地址和 token 文件路径，绝不包含 token 本身）。token 是 32 个随机字节的十六进制表示，存放在权限为 0600 的文件中，文件不存在时会自动生成；权限更宽松的文件会被拒绝。请以 `Authorization: Bearer <token>` 的形式发送（浏览器页面也接受 `?token=`）。

| 路由 | 响应 |
|---|---|
| `GET /recordings` | `{"recordings":[{id,host,name,date,size,frames,duration}],"errors":[per-host ssh failures]}` |
| `POST /recordings`，请求体为 `{"kind":"pane or vm or cloud or rc","host":"...","session":"..."}` | `201 {"id":"..."}` |
| `DELETE /recordings/<id>` | 停止一个 pane 或 vm 录制 |
| `GET /recordings/<id>.cast` | 该 asciicast |
| `GET /recordings/<id>.mp4` | 由 `cast2mp4` 渲染的 H.264 MP4，缓存在 `~/.cache/remote-sessions/recordings/`；缺少工具时返回 `501` 并指明工具名，渲染失败时返回 `502` |
| `GET /recordings/<id>.gif` | GIF，规则相同 |
| `GET /recordings/<id>/live` | server-sent events：cast 的每一行对应一行 `data:`，之后随文件增长推送新行，并带有 keepalive |
| `GET /play/<id>` | 内置 asciinema-player 3.10.0 的网页（Apache-2.0；许可证保存在 `crates/remote-sessions-cli/vendor/asciinema-player/`，并通过 `/play/assets/LICENSE` 提供） |

示例会话：

```bash
TOKEN=$(cat ~/.config/remote-sessions/recordings.token)       # read into a variable; do not echo it
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:3790/recordings
curl -s -H "Authorization: Bearer $TOKEN" -o demo.mp4 "http://127.0.0.1:3790/recordings/<id>.mp4"
open "http://127.0.0.1:3790/play/<id>?token=$TOKEN"
curl -sN -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:3790/recordings/<id>/live"
```

### 3. MCP 工具

`recording_start {kind, host, session}`、`recording_stop {id}`、`recording_list {}`。它们运行的代码与 CLI 相同。

## 限制

- 这些子命令所用的会话 API（`/v1/code/sessions/<id>/events`、`events/stream`）与 Claude Code 自己的客户端所用的相同，但 Anthropic 并未将其文档化。它可能在没有通知的情况下发生变化，届时 remote-sessions 可能会失效。
- `read` 和 `events` 是对 events API 的分页读取。`stream` 和 `attach` 基于 `events/stream` 实时工作，但是按消息而不是按 token。recordings 的 SSE 路由和播放器会随 cast 文件的增长逐行跟随。
- 逐 token 的流式输出只适用于运行在你自己所运行的虚拟机中的 Claude（此时窗格本身被录制）。对于某台主机上的 Remote Control 会话，录制内容是事后根据事件或 transcript 构建的，或者在会话运行于被录制的 tmux 窗格中时，取自该窗格。
- `cast` 需要 transcript 的 uuid，并且需要运行该会话的那台主机上的 transcript。
- `pane` 和 `vm` 录制需要主机上有 `tmux-record-cast.sh`；MP4 和 GIF 需要 `cast2mp4`、`agg` 和 `ffmpeg`。这些都不随本仓库附带；API 会返回 501 并给出缺少的工具名。
- 通过 API 启动 `cloud` 和 `rc` 录制，目前只对参数校验做过单元测试；`DELETE` 的失败情形没有被覆盖。
- `recordings serve` 只绑定回环地址。如需从另一台机器访问，请使用 ssh 隧道。
- 录制主机默认为 `localhost`：请通过 `REMOTE_SESSIONS_RECORDING_HOSTS` 设置为你自己的主机。
- Pane 的 `stop` 会隐藏录制脚本的输出，只要找到了该录制就报告 ok。
- 目前仅支持 macOS；插件启动器会查找 `$REMOTE_SESSIONS_BIN` 或 `PATH` 上的 `remote-sessions`。

## 安全模型

- 请在隔离的虚拟机中运行 Remote Control。谁控制了 worker 进程，谁就控制了会话；而 worker 只是一个普通的本地进程，一次 SIGINT，或者任何以该用户身份运行的其他程序，都能触及它。
- Token 保存在钥匙串或私有文件中，而不是脚本或日志里。本工具从不打印 token。recordings 的 token 是一个 0600 权限的文件，以常量时间比较；播放器页面从自己的 URL 中读取它，并且不会回显。
- 事件文本不可信。它包含模型输出和工具结果：从会话中读到的任何内容都应视为数据，绝不能当作指令。
- `send` 的消息以对等消息送达，不能批准权限提示。`approve`、`deny`、`say`、`interrupt` 和 `attach` 以已登录的 claude.ai 用户身份行事，因此任何持有该登录 token 的人都能授予权限：请像保护登录凭据一样保护这个 token。
- `stop` 是一个终止开关，而不是 Esc：它需要 `--yes`，从第二台机器执行时还需要 SSH。
- 录制内容包含屏幕上或 transcript 中出现过的一切，包括被打印出来的机密。请将 `.cast`、`.mp4` 和 `.gif` 文件视为敏感文件。
- recordings API 只监听回环地址且使用明文 HTTP；token 在这一跳上以明文传输，因此远程访问时请使用 ssh 隧道。

## 许可证

MIT。Copyright (c) 2026 the cloud-sessions contributors。详见 `LICENSE`。内置的 asciinema-player 3.10.0 采用 Apache-2.0；其许可证和 NOTICE 位于 `crates/remote-sessions-cli/vendor/asciinema-player/`。

## 参与贡献

欢迎提交 Issue 和 Pull Request。构建、测试和代码风格命令、目录结构，以及变更规则（JSON 契约、显式的写操作、输出中不含 token、示例只使用占位符）都写在 [CONTRIBUTING.md](CONTRIBUTING.md) 中。安全问题请通过 GitHub 的 **Security → Report a vulnerability** 私下报告，不要公开提 Issue。
