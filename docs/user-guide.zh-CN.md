# Codex Meter 中文使用指南

Codex Meter 是一个独立于 Codex CLI 的本地统计工具。它读取当前操作系统用户的 Codex 使用记录，帮助你查看 Token、项目、模型、缓存、延迟、周额度和 API 等价价格。

它不会替换官方 `codex` 命令，也不会清除或修改原来的 Codex 会话。

![Codex Meter 主界面](assets/dashboard.png)

## 1. 安装

### Linux 和 macOS

打开终端，粘贴下面这一行：

```bash
curl -fsSL https://raw.githubusercontent.com/DelicateNorman/codex-meter/v0.14.2/install.sh | sh
```

安装完成后运行：

```bash
codex-meter
```

程序默认安装到当前用户的 `~/.local/bin`，不需要 `sudo`。

### Windows PowerShell

打开 PowerShell，粘贴下面这一行：

```powershell
irm https://raw.githubusercontent.com/DelicateNorman/codex-meter/v0.14.2/install.ps1 | iex
```

安装后重新打开一个终端，再运行：

```powershell
codex-meter
```

Windows 版本默认安装到：

```text
%LOCALAPPDATA%\Programs\CodexMeter\bin
```

### 检查是否安装成功

```bash
codex-meter --version
```

当前版本应显示：

```text
codex-meter 0.14.2
```

如果提示找不到命令，请关闭当前终端并重新打开。Linux/macOS 还可以检查：

```bash
command -v codex-meter
```

正常情况下会显示类似：

```text
/home/你的用户名/.local/bin/codex-meter
```

## 2. 第一次打开

直接运行：

```bash
codex-meter
```

程序会完成三件事：

1. 扫描当前用户 `~/.codex/sessions` 下新增或发生变化的 Rollout 文件；
2. 把不包含对话正文的统计信息写入本地数据库；
3. 打开可用方向键操作的交互界面。

已经导入且没有变化的文件会被跳过，因此后续启动不需要重新解析全部历史。

周额度会在后台读取。界面可能先短暂显示：

```text
ACCOUNT WEEKLY LIMITS  Loading…
```

Codex 返回额度后，程序会自动刷新成柱状条，不需要按键。

## 3. 界面怎么操作

主菜单位于页面底部。

| 按键 | 作用 |
|---|---|
| `↑` `↓` `←` `→` | 在底部菜单中移动 |
| `Enter` 或 `Space` | 打开选中的页面 |
| `/` | 打开命令搜索面板 |
| `Esc` | 关闭命令面板、项目选择或帮助 |
| `r` | 重新读取本地记录和周额度 |
| `q` | 在主页面退出程序 |
| `Ctrl+C` | 退出程序 |

### 为什么有时按 `q` 不退出？

在 `/` 命令面板和 Project 搜索框里，`q` 会被当作输入文字。这是为了让项目名和命令可以正常包含字母 `q`。

先按 `Esc` 回到主页面，再按 `q` 即可退出。

### `/` 命令面板

按 `/` 后可以直接输入命令，也可以用上下键选择：

```text
/today
/week
/month
/all
/history day
/history week
/history month
/network
/project
/refresh
/help
/quit
```

每条命令旁边都有一行英文功能说明。按 `Enter` 执行，按 `Esc` 返回。

## 4. 每个页面统计什么

### Today

当前本地日期的使用情况。

### Week

当前自然周的本地使用情况。自然周按周一到周日计算。

### Month

当前自然月的本地使用情况。

### All time

从已导入的第一条 Codex 使用记录开始，统计到现在。

### Daily / Weekly / Monthly history

分别按日、周、月列出历史统计，适合观察长期变化。

### Network

显示当前已收集到的响应性能，包括：

- TTFT：从请求开始到第一个输出 Token 的时间；
- E2E：一次请求从开始到结束的总时间；
- Output TPS：每秒输出 Token 数；
- 最近的网络连接、字节数、状态和延迟。

不同 Codex 记录包含的时间字段不同。如果原始记录没有足够信息，某些速度或延迟会显示 `N/A` 或 `estimated`，而不是编造结果。

### Project

切换项目统计范围。默认是 `All projects`，即统计当前 OS 用户的全部 Codex 项目。

项目列表按最近使用时间排序。打开 Project 后可以直接输入文字过滤，使用上下键选择，按 `Enter` 应用，按 `Esc` 取消。

项目名来自 Rollout 工作目录的最后一级目录名。两个路径如果最后一级名字相同，会被归为同一个项目。

## 5. 周额度和本地 Week 的区别

这是最容易混淆的地方。

### ACCOUNT WEEKLY LIMITS

这是当前登录 Codex 账号由后端返回的真实七日额度，例如：

```text
Codex · 82% left · reset Aug 18 11:42
Used  ███████░░░░░░░░░░░░░░░░░░░░░░░░░ 18%
```

- `18%` 表示该额度周期已经使用 18%；
- `82% left` 表示还剩 82%；
- `reset` 表示额度重置时间，使用本机时区显示；
- 如果账号返回多个额度桶，程序会分别显示，例如 Codex 和 GPT-5.3-Codex-Spark。

这个额度属于当前 Codex 账号，不会随 Day、Week、Month 或 Project 的选择而变化。

### Week 页面

Week 是从本机历史记录计算出的当前自然周用量。它是分析报表，不是账号额度。

因此：

- 顶部绿色额度条回答“账号七日额度还剩多少”；
- Week 页面回答“本机这个自然周记录了多少使用量”。

两者的时间窗口和数据来源不同，不应该期待数值相等。

## 6. Token 区域怎么看

### 顶部指标

| 字段 | 含义 |
|---|---|
| `TOKENS` | 当前范围的总 Token 数 |
| `API-EQUIV` | 按公开 API 价格估算的等价金额，不是订阅账单 |
| `CACHE` | 缓存输入占全部输入的比例 |
| `CALLS` | 识别到的模型调用次数 |
| `Input` | 输入 Token |
| `Output` | 输出 Token |
| `Reasoning` | 输出中属于推理部分的 Token |
| `Cache read` | 从缓存复用的输入 Token |
| `Cache miss` | 没有命中缓存的输入 Token |
| `Cache write` | 写入缓存的 Token |

### 三条 Token 柱

- `Input total`：输入 Token 总量，不显示无意义的 100%；
- `Cached input`：缓存输入 ÷ 全部输入；
- `Reasoning out`：推理 Token ÷ 全部输出。

例如：

```text
Cached input  96.7% of input
Reasoning out 29.7% of output
```

这两个百分比的分母不同，不能直接相互比较。

### 为什么价格有 `N/A`？

`N/A` 表示该模型在当前价格表中没有可信价格。常见情况包括：

- 内部模型；
- 自动审查模型；
- 新模型还没有加入价格表；
- 原始记录没有准确模型名。

Codex Meter 会把这些调用计入 Token 和次数，但不会为它们编造价格。顶部黄色提示会说明有多少调用未计价。

## 7. 按项目统计

交互界面中使用 Project 菜单最方便。命令行也可以直接指定项目：

```bash
codex-meter today --project codex-stats
codex-meter summary --period week --project codex-stats
codex-meter summary --period month --project codex-stats
codex-meter history --group month --project codex-stats
```

查看项目汇总：

```bash
codex-meter projects
```

不添加 `--project` 时始终统计全部项目。

## 8. 按账号标签统计

账号标签默认关闭。它不会自动读取真实账号邮箱或认证信息，而是由你手动设置一个本地名称。

查看状态：

```bash
codex-meter account status
```

启用并设置标签：

```bash
codex-meter account enable personal
```

以后切换标签：

```bash
codex-meter account set work
```

查看已有标签：

```bash
codex-meter account list
```

按标签统计：

```bash
codex-meter summary --period month --account work
```

关闭标签：

```bash
codex-meter account disable
```

标签主要应用于之后导入的会话。原来没有标签的历史记录会保持 `Unassigned`。只有在你确认所有未分配历史都属于同一账号时，才使用：

```bash
codex-meter account claim-unassigned personal
```

## 9. 不进入交互界面直接查询

### 某一天

```bash
codex-meter summary --period day --date 2026-08-12
```

### 某一周

`--date` 可以是该周内任意一天：

```bash
codex-meter summary --period week --date 2026-08-12
```

### 某个月

```bash
codex-meter summary --period month --date 2026-08-12
```

### 全部历史

```bash
codex-meter summary --period all
```

### 纯文本输出

`--no-color` 必须写在子命令前面：

```bash
codex-meter --no-color summary --period week
```

## 10. 导出统计

导出 CSV：

```bash
codex-meter export \
  --from 2026-08-01 \
  --to 2026-08-12 \
  --format csv \
  --output usage.csv
```

导出 JSONL：

```bash
codex-meter export --format jsonl --output usage.jsonl
```

导出某个会话：

```bash
codex-meter export --session SESSION_ID --format json
```

导出内容是使用和性能元数据，不包含提示词、回复正文或工具输出。

## 11. Network 和抓包功能

### 查看已经保存的网络记录

```bash
codex-meter network show
```

### 测试 DNS、TCP 和 TLS 建连

```bash
codex-meter network probe api.openai.com
```

### 被动抓取包长度和方向

```bash
codex-meter network capture \
  --host api.openai.com \
  --host chatgpt.com \
  --duration 15
```

这个模式不使用 `tcpdump -A`、`-X` 或 `-w`，只记录：

- 目标地址；
- 数据方向；
- 包数量和长度；
- 持续时间。

它不会保存数据包正文，但操作系统仍可能要求 tcpdump 权限。Linux 和 macOS 会自动选择常见抓包接口；Windows 被动抓包需要兼容的 tcpdump 环境。

### 不解密 TLS 的 CONNECT 代理

```bash
codex-meter proxy tunnel --port 8899
```

### HTTP/WebSocket 反向代理

```bash
codex-meter proxy reverse \
  --port 8900 \
  --upstream https://chatgpt.com/backend-api/codex
```

请求和响应内容只在内存中转发，数据库只保存状态、时间和字节数。

### 显式 TLS 终止诊断

这是独立的高级模式，只有明确需要时才使用：

```bash
codex-meter proxy tls-init
codex-meter proxy tls --acknowledge-sensitive \
  --upstream https://chatgpt.com/backend-api/codex
```

程序会在 `~/.codex-meter/tls` 创建短期本地 CA 和证书。只在诊断期间信任该 CA，结束后应删除系统中的信任。即使开启此模式，Codex Meter 也不会把请求头、正文、SSE 或 WebSocket 帧写入数据库。

## 12. 可选的实时性能数据

普通用户只使用 Rollout 历史即可。下面功能用于补充更准确的延迟、吞吐或调用生命周期。

### OTLP

生成 Codex 配置片段：

```bash
codex-meter otel config
```

把输出复制到 `~/.codex/config.toml`，然后在启动 Codex 前运行：

```bash
codex-meter otel serve
```

OTLP 收集器只保留允许列表中的统计字段，不保存提示词或任意事件正文。

### App Server 代理

```bash
codex-meter app-server proxy
```

导入已有 App Server JSONL：

```bash
codex-meter app-server ingest FILE
```

这可以补充精确的单次响应用量、Turn 生命周期、工具类型与时间、reroute 和 compaction 等信息。

## 13. 数据保存在哪里

默认目录：

```text
~/.codex-meter/
├── meter.db
├── config.toml
├── pricing.json
└── logs/
```

其中 `meter.db` 是主要统计数据库。

Codex Meter 只读取当前用户的：

```text
~/.codex/sessions/
```

不会扫描其他操作系统用户的 Home 目录。

### 备份

关闭 Codex Meter 后复制数据库即可：

```bash
cp ~/.codex-meter/meter.db ~/codex-meter-backup.db
```

### 使用其他数据目录

临时指定：

```bash
codex-meter --home /path/to/meter-data
```

或设置环境变量：

```bash
export CODEX_METER_HOME=/path/to/meter-data
```

## 14. 更新和卸载

### 更新

从 [Releases 页面](https://github.com/DelicateNorman/codex-meter/releases) 找到最新版本，重新运行对应版本的一行安装命令即可。

安装器只替换程序文件，不会删除 `~/.codex-meter/meter.db`。

### 只卸载程序，保留统计记录

Linux/macOS：

```bash
rm ~/.local/bin/codex-meter
```

Windows PowerShell：

```powershell
Remove-Item "$env:LOCALAPPDATA\Programs\CodexMeter\bin\codex-meter.exe"
```

这不会删除数据库。如果以后重新安装，原来的统计仍然存在。

只有确定不再需要任何统计历史时，才手动删除 `~/.codex-meter`。

## 15. 常见问题排查

### 看不到周额度

1. 确认版本：

   ```bash
   codex-meter --version
   ```

2. 等待 `Loading…` 自动刷新；额度读取不会阻塞界面。
3. 按 `r` 重试。
4. 查看页面上的具体 `Unavailable` 原因。
5. 确认官方 `codex` 命令可以正常运行且当前账号已登录。

### 没有新的历史记录

在 Codex 中完成一次真实请求后，回到 Codex Meter 按 `r`。也可以运行：

```bash
codex-meter import ~/.codex/sessions
```

### 打开仍然是旧版本

```bash
command -v codex-meter
codex-meter --version
```

如果命令路径不是当前用户的 `~/.local/bin/codex-meter`，说明 PATH 中存在另一个同名程序。

### 终端显示不完整

扩大终端高度可以看到更多模型行。顶部摘要和周额度会优先保留；窄终端会自动使用紧凑布局。

### 数据有问题

先运行：

```bash
codex-meter doctor
```

如果需要反馈问题，请同时提供：

- `codex-meter --version`；
- 操作系统；
- `codex-meter doctor` 输出；
- 不包含隐私内容的界面截图。

不要上传 `auth.json`、访问令牌、完整 Rollout、提示词或回复正文。

## 16. 从源码运行

需要 Python 3.11 或更高版本：

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -e .
python -m unittest discover -v
codex-meter
```

Windows PowerShell 激活命令：

```powershell
.\.venv\Scripts\Activate.ps1
```

更完整的构建说明见 [从源码构建](build-from-source.md)。

## 17. 隐私原则

Codex Meter 的设计原则是“统计元数据，不保存内容”。

它不会持久化：

- 提示词、模型回复和推理正文；
- shell 命令、工具参数和工具输出；
- HTTP 请求头、Cookie、认证信息；
- SSE 数据和 WebSocket 帧；
- Codex 账号邮箱或 `auth.json`。

所有功能都应该遵守这个原则。如果你发现任何可能保存正文或凭据的情况，请停止使用相关功能并提交 Issue。
