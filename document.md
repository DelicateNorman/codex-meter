# Codex CLI Usage Observatory

## 软件需求分析与工程实现建议

## 1. 项目定位

开发一款能够**嵌入 Codex CLI 使用流程**的本地用量分析与性能观测工具。

产品暂定名：

**Codex Meter / Codex Scope / Codex Lens**

核心定位不是简单统计 Token，而是：

> 为 Codex CLI 提供类似“开发者性能监控面板 + Token 成本分析 + 模型性能分析”的本地 Observability Layer。

工具需要能够回答：

* 我今天到底用了多少 Token？
* 哪个模型用得最多？
* GPT-5.6 / GPT-5.x 在不同 thinking/reasoning level 下用了多少？
* high / xhigh 到底比 medium 多烧多少 Token？
* reasoning token 占了多少？
* 缓存命中了多少？
* 哪些请求缓存没有命中？
* Cache Write 花了多少？
* 如果没有缓存，本来要多花多少钱？
* 哪个模型最贵？
* 哪个模型响应最快？
* 从发送请求到首 Token 等了多久？
* 模型真正开始输出后速度是多少 Token/s？
* 一次 Codex Turn 为什么花了 40 秒？
* 是模型慢、网络慢、thinking 慢，还是 shell/tool 慢？
* Codex 为完成一次用户任务实际上调用了多少次模型？
* retry / reconnect / compaction / tool loop 浪费了多少 Token？
* 哪些项目、Session、Turn 最烧钱？
* 今天相当于消耗了多少美元？

---

# 2. 设计原则

## 2.1 Local First

默认全部统计数据保存在本机。

不得依赖 SaaS 后端。

不得为了统计数据，把 prompt、代码、API Key、Authorization Header 上传到第三方服务器。

默认：

```text
~/.codex-meter/
    meter.db
    config.toml
    pricing.json
    logs/
```

---

## 2.2 不只统计 Session，要统计真正的 LLM Call

这是整个项目最重要的设计要求。

Codex 是 Agent。

一次：

```text
用户输入
    ↓
Codex Turn
```

内部可能实际上发生：

```text
LLM Call #1
↓
Tool Call
↓
LLM Call #2
↓
Shell
↓
LLM Call #3
↓
Tool
↓
LLM Call #4
↓
最终回复
```

因此数据库必须按照：

```text
Session
  └── Turn
       ├── LLM Call
       ├── Tool Call
       ├── LLM Call
       ├── Tool Call
       └── LLM Call
```

进行建模。

**绝对不能只保存一个 Turn 总 Token。**

否则：

* TTFT 算不准
* Token/s 算不准
* thinking 分析算不准
* Tool 等待算不准
* Retry 成本算不准
* Cache 分析算不准

---

# 3. 当前 Codex 可利用的数据源

工程实现应采取“多数据源融合”，而不是依赖单一接口。

优先级：

```text
P0  Codex OpenTelemetry
P0  Codex App Server Events
P1  Codex Session JSONL
P1  Codex 内部事件（如果直接 fork Codex）
P2  本地 Network Probe
P3  可选 Local Proxy / MITM Diagnostic Mode
```

---

# 4. 第一数据源：Codex OpenTelemetry

Codex 当前已经支持 OpenTelemetry，而且能够输出 API 请求、SSE/WebSocket、Turn、Tool 等观测数据。官方目前公开的指标已经包括：

* API request duration
* SSE event
* WebSocket request/event
* Responses API inference time
* Responses API overhead
* engine TTFT
* TBT
* Turn E2E duration
* Turn TTFT
* Turn TTFM
* Turn token usage
* Tool call duration

其中 `turn.token_usage` 已区分：

```text
total
input
cached_input
output
reasoning_output
```

因此第一版优先利用 OTel，而不是自己重复造一整套计时器。

建议：

```text
Codex
   ↓ OTLP
Codex Meter Local Collector
   ↓
Normalizer
   ↓
SQLite
   ↓
Metric Engine
   ↓
TUI Dashboard
```

Collector 应监听 localhost，例如：

```text
127.0.0.1:4318
```

只接受本机 Codex 数据。

---

# 5. 第二数据源：Codex App Server

Codex App Server 当前会实时发送：

```text
thread/tokenUsage/updated
turn/started
turn/completed
item/started
item/.../delta
item/completed
```

实验模式还存在：

```text
rawResponse/completed
```

其中 `rawResponse/completed` 可以按**每一次上游 Responses API completion**给出 usage，而不是仅给累计 Session Token。官方也明确说明它与累计型 `thread/tokenUsage/updated` 不同。

因此建议做一个：

```text
AppServerCollector
```

直接消费 JSON-RPC event stream。

特别重要的是：

```text
responseId
threadId
turnId
```

必须全部保留。

这样才能实现：

```text
Turn → 多个 Response / LLM Call
```

映射。

由于 `rawResponse/completed` 当前属于 experimental/internal 能力，所以代码必须使用 Adapter：

```rust
trait UsageCollector {
    fn capabilities(&self) -> Capabilities;
    fn start(&self);
}
```

不能把整个项目写死在一个实验字段上。

---

# 6. 第三数据源：Session JSONL

本地 Session 历史数据用于：

* 首次安装时历史回溯
* Codex Meter 没启动时补数据
* 数据校验
* Reasoning token 补充
* Debug

Codex 的 session 记录中已经出现：

```text
input_tokens
cached_input_tokens
output_tokens
reasoning_output_tokens
total_tokens
```

并存在：

```text
last_token_usage
total_token_usage
```

两种语义，因此解析时不能把累计值重复相加。

必须做：

```text
deduplication
delta detection
event id / timestamp reconciliation
```

否则非常容易出现 Token 翻倍。

---

# 7. 数据模型

建议 SQLite。

至少建立以下表。

## sessions

```text
id
codex_thread_id
started_at
ended_at
cwd
project_name
git_repo
git_branch
auth_mode
codex_version
source
```

---

## turns

```text
id
session_id
codex_turn_id
started_at
completed_at
status

model
reasoning_effort
reasoning_mode
service_tier

input_tokens
cached_input_tokens
cache_write_tokens
output_tokens
reasoning_tokens
total_tokens

cost_usd

ttft_ms
ttfm_ms
e2e_ms
tool_time_ms
model_time_ms
```

---

## llm_calls

这是核心表。

```text
id

session_id
turn_id
response_id

started_at
first_event_at
first_model_item_at
first_visible_token_at
last_token_at
completed_at

model
actual_model
provider

reasoning_effort
reasoning_mode

transport
service_tier

input_tokens
cached_input_tokens
cache_write_tokens
output_tokens
reasoning_tokens
total_tokens

request_duration_ms
ttfb_ms
ttfm_ms
ttft_ms
generation_ms
inference_ms
overhead_ms
avg_tbt_ms

output_tps
visible_output_tps

retry_index
success
error_type

cost_usd
```

---

## tool_calls

```text
id
turn_id
llm_call_id

tool_name
started_at
completed_at
duration_ms

success
exit_code
```

例如：

```text
shell
apply_patch
web_search
MCP
file read
subagent
```

---

## pricing_snapshots

```text
model
provider
effective_from

input_per_million
cached_input_per_million
cache_write_per_million
output_per_million

long_context_threshold
long_context_input
long_context_cached
long_context_cache_write
long_context_output

currency
pricing_version
```

**不要把模型价格硬编码进计算逻辑。**

必须做成数据驱动。

---

# 8. Token 分析

必须至少展示：

```text
Total Tokens
Input Tokens
Output Tokens
Reasoning Tokens
Cached Input Tokens
Cache Write Tokens
Cache Miss Tokens
```

定义：

```text
cache_read_hit_tokens = cached_input_tokens

cache_read_miss_tokens =
    input_tokens - cached_input_tokens
```

这里的 Miss 指：

> 本次 Input 中没有从 Prompt Cache 读取的 Token。

GPT-5.6 及之后的模型还需要额外保存：

```text
cache_write_tokens
```

OpenAI 当前 Responses usage 已提供：

```text
input_tokens_details.cached_tokens
input_tokens_details.cache_write_tokens
output_tokens_details.reasoning_tokens
```

因此缓存分析不能再简单分：

```text
cached / uncached
```

而应该至少分成：

```text
Cache Read
Cache Miss
Cache Write
```

---

# 9. Cache 分析

核心指标：

### Cache Hit Rate

```text
cached_input_tokens / input_tokens
```

### Cache Miss Rate

```text
1 - cache_hit_rate
```

### Cache Write Rate

```text
cache_write_tokens / input_tokens
```

### Cache Reuse Efficiency

建议增加窗口指标：

```text
subsequent_cached_reads / previous_cache_writes
```

用于判断：

> 写进去的 Cache 后面到底有没有真正复用。

---

## Cache Savings

增加：

```text
Cost Without Cache
Actual Input Cost
Cache Savings
```

例如：

```text
Without cache: $2.31
Actual:        $0.61
Saved:         $1.70
Save rate:     73.6%
```

GPT-5.6 系列当前已经区分 cached read 与 cache write 价格，cache write 使用独立价格；官方定价页面也区分 Input、Cached Input、Cache Writes、Output。

因此 Pricing Engine 必须支持至少：

```text
normal input
cache read
cache write
output
```

四种价格。

---

# 10. Reasoning / Thinking Level 分析

模型不要只按照：

```text
gpt-5.6
```

统计。

必须按照：

```text
Model × Reasoning Effort × Reasoning Mode
```

组合分析。

例如：

```text
GPT-5.6
 ├── low
 ├── medium
 ├── high
 └── xhigh
```

未来可能还有：

```text
minimal
max
none
```

因此数据库中的 reasoning effort **必须保存 raw string，禁止写死 enum 范围**。

当前 reasoning effort 的可选值是 model-dependent，可包括 `none / minimal / low / medium / high / xhigh / max`；GPT-5.6 还区分 standard/pro reasoning mode，并且 mode 与 effort 是两个不同维度。

Dashboard 展示：

```text
GPT-5.6

Effort     Calls    Tokens     Reasoning    Cost       TTFT     TPS
-------------------------------------------------------------------
low          124     4.1M       0.42M       $8.21      1.7s    82
medium       218    11.7M       2.81M      $27.30      3.2s    61
high          87     9.2M       3.92M      $25.71      6.8s    44
xhigh         31     6.9M       4.11M      $21.42     11.4s    31
```

这样才能真正回答：

> xhigh 是否值得。

---

# 11. Thinking 时间

这里必须定义清楚。

不要把：

```text
请求开始 → 首 Token
```

直接命名成：

```text
Thinking Time
```

因为其中可能包含：

```text
网络
排队
prompt processing
cache lookup
推理
服务端调度
```

而且 OpenAI reasoning tokens 可以计数，但隐藏 reasoning 内容本身并不会因此变成可读取的 Chain-of-Thought。

因此 UI 使用三个指标：

### ① TTFT

```text
Request Start
      ↓
First Visible Token
```

---

### ② Pre-output Wait

```text
first_visible_token_at - request_started_at
```

UI：

```text
Pre-output wait: 4.31s
```

可以俗称“等待模型思考时间”，但内部字段不得叫 exact_thinking_time。

---

### ③ Observed Reasoning Phase

如果某模型/provider 确实产生 reasoning item / reasoning summary：

```text
first_reasoning_event
        ↓
first_visible_answer
```

记录：

```text
observed_reasoning_phase_ms
```

如果没有对应事件：

```text
NULL
```

**禁止伪造。**

---

# 12. Latency 分析

这是本工具非常重要的差异化功能。

一个 Turn 要拆解成：

```text
User submits
      │
      ├─ client overhead
      │
      ├─ network
      │
      ├─ API request
      │
      ├─ model inference / reasoning
      │
      ├─ first output
      │
      ├─ token streaming
      │
      ├─ tool execution
      │
      ├─ next LLM call
      │
      └─ final completion
```

记录：

```text
Turn E2E
Request Duration
TTFM
TTFT
Inference Time
API Overhead
Generation Duration
Average TBT
Tool Duration
```

Codex OTel 当前已经提供其中多项底层指标，包括 Responses API inference、overhead、engine TTFT/TBT、turn TTFT/TTFM/e2e。

---

# 13. Token/s

至少提供两种速度。

## Generation TPS

```text
output_tokens
/
generation_duration
```

---

## Visible TPS

建议：

```text
visible_output_tokens
/
(last_visible_token_at - first_visible_token_at)
```

如果无法取得精确 visible token：

显示：

```text
~ 63 tok/s
```

明确表示为估算值。

不要简单：

```text
output_tokens / entire request duration
```

因为 reasoning 等待时间会严重扭曲“吐 Token 速度”。

---

# 14. Percentile

平均值意义有限。

所有性能指标至少提供：

```text
P50
P90
P95
P99
```

例如：

```text
GPT-5.6 medium

TTFT
P50   2.8s
P90   5.2s
P95   7.1s
P99  13.4s
```

TPS 同样如此。

---

# 15. USD 成本

必须区分两个概念。

## API Key 模式

显示：

```text
Estimated API Cost
```

根据：

```text
model
tokens
cache
service tier
context length
pricing snapshot
```

计算。

---

## ChatGPT 登录 Codex

这里不能把结果写成：

```text
You spent $12.73
```

因为 ChatGPT 套餐 Codex 的消耗不等于 API 按 Token 实际扣款。

应该显示：

```text
API-equivalent value
```

例如：

```text
Equivalent API value today
$18.73
```

即：

> 如果按照对应模型公开 API 单价计算，相当于多少美元。

---

# 16. Reasoning Token 成本

注意：

Reasoning Token 是：

```text
output_tokens
```

的一部分。

OpenAI 官方说明 reasoning tokens 计入 output token，并按照 output token 计费。

因此：

```text
output cost
```

不能再额外把 reasoning token 加一次。

否则会 double count。

但 UI 可以展示：

```text
Output Tokens       12,300
├ Reasoning          7,900
└ Other Output       4,400
```

用于分析组成。

---

# 17. 推荐增加的高级指标

这些是非常值得做的。

## 17.1 Context Amplification

```text
LLM Input Tokens
/
User Prompt Tokens
```

例如用户只输入：

```text
82 tokens
```

最终模型调用：

```text
64,000 input tokens
```

显示：

```text
Context amplification: 780×
```

这对发现 Codex Session 越聊越贵非常有价值。

---

## 17.2 Reasoning Ratio

```text
reasoning_tokens / output_tokens
```

---

## 17.3 Tool Time Ratio

```text
tool_execution_time / turn_e2e_time
```

判断：

> 慢的是模型还是 shell/MCP？

---

## 17.4 Retry Tax

统计：

```text
retry token
retry request
reconnect
failed request
fallback
```

显示：

```text
Retry tax today

Tokens wasted: 182K
Equivalent: $0.73
Time wasted: 74 sec
```

---

## 17.5 Cache Waste

如果进行了大量 cache write，但后面几乎没有 cache read：

```text
Cache write: 3.8M
Later reused: 0.4M

Reuse efficiency: 10.5%
```

显示警告：

```text
Low cache reuse
```

---

## 17.6 Cost Velocity

```text
USD equivalent / hour
```

比如：

```text
Current burn rate
$4.82 / hour
```

---

## 17.7 Cost per Successful Turn

```text
total cost / successful turns
```

---

## 17.8 Model Efficiency

对比：

```text
Model
Reasoning
Cost/Turn
TTFT
TPS
Reasoning Tokens
Success Rate
```

以后甚至可以支持：

```text
Which model gives the best speed/cost balance?
```

---

# 18. Compaction 分析

Codex 长 Session 会发生 context compaction。

必须单独统计：

```text
Compaction Count
Tokens before compaction
Tokens after compaction
Compaction Token Cost
Time Spent
```

OTel 当前也存在 `task.compact` 指标，因此可以关联 compaction 行为。

---

# 19. Subagent 分析

未来必须支持：

```text
Main Agent
 ├─ Agent A
 ├─ Agent B
 └─ Agent C
```

统计：

```text
Agent
Model
Tokens
Reasoning
Cost
Duration
Tool Calls
```

否则随着 Codex multi-agent 使用增加，主线程统计会越来越失真。

当前 Codex telemetry 已经存在 multi-agent spawn/resume 等指标，因此数据模型现在就应保留：

```text
parent_thread_id
agent_role
agent_id
```

扩展字段。

---

# 20. Dashboard

整体视觉：

**深色蓝色系、偏开发者工具风格。**

推荐颜色：

```text
Background      #07111F
Panel           #0B1F33
Primary Blue    #0A84FF
Cyan            #38BDF8
Light Blue      #60A5FA
Text Primary    #EAF2FF
Text Secondary  #93A4B8
Success         #38D996
Warning         #F7C948
Danger          #FF647C
```

不要做得花哨。

风格类似：

```text
terminal
+
Grafana
+
developer profiler
```

---

# 21. Codex CLI 内嵌 UI

最终希望提供：

```text
/meter
```

或：

```text
/usage
```

打开 Overlay。

Codex CLI 当前已经支持 `/statusline` 并能展示 model、reasoning、context、rate limits、token counters 等状态项，所以我们可以复用现有 TUI 状态模型，而不必完全重新设计底部状态系统。

建议新增：

```text
/model-usage
/perf
/meter
```

其中 `/meter` 为完整 Dashboard。

---

# 22. 默认 Footer

底部保持非常克制：

```text
5.6 medium │ 84.2K tok │ 71% cache │ $0.42 eq │ TTFT 2.8s │ 64 tok/s
```

蓝色系显示。

不要一直展示几十个指标。

---

# 23. /meter 主界面

示意：

```text
╭─ CODEX METER ─────────────────────────────────────────────────────╮
│ LIVE ●   GPT-5.6 · medium     Session 01J...          13:42:18    │
├────────────────────────────────────────────────────────────────────┤
│                                                                    │
│  TOKENS             COST              CACHE            PERFORMANCE │
│  184.3K             $0.84 eq           78.2%            TTFT 2.81s │
│  ↑ 28.4K this turn  $0.13 this turn    144K cached      67 tok/s  │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ Input                                                            │
│ ███████████████████████████████████████████  163.2K               │
│                                                                    │
│ Cached                                                            │
│ ██████████████████████████████████████       127.6K   78.2%       │
│                                                                    │
│ Reasoning                                                         │
│ ███████████                                  19.4K                 │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ Model                 Tokens      Cache    Cost      TTFT     TPS   │
│ gpt-5.6 medium        101.2K       82%    $0.41      2.8s     67   │
│ gpt-5.6 high           61.7K       74%    $0.35      5.9s     49   │
│ gpt-5.6 xhigh          21.4K       53%    $0.08     10.2s     32   │
╰────────────────────────────────────────────────────────────────────╯
```

---

# 24. 页面结构

提供 Tabs：

```text
Overview
Models
Reasoning
Cache
Latency
Sessions
Projects
Calls
Tools
Network
Raw
```

快捷键：

```text
1 Overview
2 Models
3 Cache
4 Performance
5 Sessions

Tab next
Shift+Tab previous
Esc close
r refresh
e export
```

---

# 25. Models 页面

显示：

```text
Model        Effort   Calls   Input   Cache   Reason   Output   Cost
```

支持：

```text
↑ ↓ sort
```

按：

```text
Cost
Tokens
Calls
Latency
TPS
```

排序。

---

# 26. Cache 页面

重点展示：

```text
Cache Hit
Cache Miss
Cache Write
Cache Savings
Reuse Efficiency
```

增加时间曲线：

```text
100% ┤
 80% ┤       ╭─────╮
 60% ┤   ╭───╯     ╰─────
 40% ┤───╯
 20% ┤
  0% ┼────────────────────
```

可以直接发现：

> 为什么某一段 Session Cache 突然掉了。

---

# 27. Latency 页面

做 waterfall：

```text
Turn #128                        18.42 sec

Request #1
Network        █ 0.12
Pre-output     ██████ 3.42
Generation     ███ 1.74

Tool: shell
               █████ 2.81

Request #2
Pre-output     ████████ 4.82
Generation     ████ 2.11

Tool: apply_patch
               ██ 0.94
```

这个功能会让产品价值明显高于普通 Token counter。

---

# 28. Sessions 页面

例如：

```text
Today

Session                           Tokens      Cost       Cache
--------------------------------------------------------------
codex-meter project               2.81M       $7.82       82%
TikTok scraper                    1.13M       $3.21       54%
research                          0.82M       $2.14       91%
```

支持：

```text
Today
7 days
30 days
All
```

---

# 29. Project 维度

根据：

```text
cwd
git repository
git branch
```

自动聚合。

例如：

```text
Projects this month

codex-meter       $31.72
ecommerce-agent   $18.42
research-tools     $7.11
```

这样以后可以直接知道：

> 哪个项目最烧 Codex。

---

# 30. Network Probe

除了官方 telemetry，可以增加自己的网络观测。

分级处理。

## Level 0：Passive Network Metadata

默认可以支持：

```text
DNS time
TCP connect time
TLS handshake time
connection reuse
bytes sent
bytes received
connection reset
reconnect
```

无需读取 HTTPS 内容。

---

# 31. Local Proxy Mode

如果用户通过 API Key / OpenAI-compatible provider 使用 Codex，可以增加：

```text
codex-meter proxy
```

形成：

```text
Codex
  ↓
127.0.0.1 Codex Meter Proxy
  ↓
OpenAI / Compatible Provider
```

Codex 当前本身支持自定义 model provider、`base_url` 和 Responses wire API，因此该路线对于自有 API/provider 环境是合理的。

Proxy 可以记录：

```text
request begin
headers size
body size
response headers
first SSE event
first output delta
last delta
response.completed
usage
retry
HTTP status
```

但：

**默认绝不落盘：**

```text
Authorization
Cookie
API Key
完整 Prompt
完整模型输出
```

只落统计元数据。

---

# 32. MITM / 抓包模式

可以做，但只作为：

```text
Experimental Diagnostic Mode
```

用途：

> 分析用户自己机器上、自己授权的 Codex 网络请求。

不作为主采集方式。

原因：

```text
脆弱
容易受 TLS/OAuth 更新影响
存在凭证暴露风险
维护成本高
```

如果仅需要性能分析，应优先：

```text
OTel
App Server
source instrumentation
local proxy
```

而不是 TLS MITM。

如果未来实现 MITM：

```text
OFF by default
仅监听 localhost
不保存 Authorization
不保存 Cookie
不保存 access token
不保存完整 prompt
不保存 reasoning payload
```

并增加明显的：

```text
⚠ Diagnostic interception enabled
```

提示。

---

# 33. 如果真的想做到最强：直接 Instrument Codex Source

因为 Codex 本身是开源项目，最终最稳定的高级版本应该直接在网络请求生命周期插 Hook：

```text
before_request
after_connect
response_headers
first_sse
first_reasoning_event
first_output_delta
response_completed
retry
error
```

把数据统一送进：

```text
MeterEventBus
```

例如：

```rust
enum MeterEvent {
    TurnStarted,
    RequestStarted,
    ResponseConnected,
    FirstModelItem,
    FirstVisibleToken,
    UsageReceived,
    ToolStarted,
    ToolCompleted,
    RequestCompleted,
    TurnCompleted,
}
```

然后：

```text
TUI
SQLite
JSON export
OTel
```

全部订阅 MeterEventBus。

---

# 34. 推荐工程架构

```text
codex-meter/
│
├── crates/
│   ├── meter-core/
│   │   ├── events
│   │   ├── models
│   │   ├── normalizer
│   │   └── metrics
│   │
│   ├── meter-collector/
│   │   ├── otel
│   │   ├── app-server
│   │   ├── session-jsonl
│   │   └── network
│   │
│   ├── meter-storage/
│   │   ├── sqlite
│   │   └── migrations
│   │
│   ├── meter-pricing/
│   │   ├── catalog
│   │   └── calculator
│   │
│   ├── meter-tui/
│   │   ├── overview
│   │   ├── models
│   │   ├── cache
│   │   ├── latency
│   │   └── sessions
│   │
│   └── meter-cli/
│
└── pricing/
    └── openai.json
```

如果直接 fork Codex：

```text
openai/codex
│
├── codex-rs/
│
├── codex-meter-core
│
├── codex-meter-storage
│
└── codex-meter-tui
```

核心统计模块必须与 UI 分离。

---

# 35. Collector Adapter

统一接口：

```rust
trait Collector {
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> CollectorCapabilities;

    async fn run(
        &self,
        sender: Sender<MeterEvent>
    ) -> Result<()>;
}
```

以后可以轻松接：

```text
OpenAI
Anthropic
Gemini
OpenRouter
Azure
本地 Ollama
第三方中转
```

不要把软件写死成 OpenAI-only。

---

# 36. Event Normalization

所有来源最终转成统一事件：

```text
MeterEvent
```

例如：

```text
Raw Codex JSONL
Raw OTel
Raw AppServer
Raw Proxy
       ↓
Normalizer
       ↓
MeterEvent
       ↓
Aggregation
```

这样官方以后字段改变，只需要修改 Adapter。

---

# 37. Data Quality

每一个数据字段增加：

```text
source
confidence
estimated
```

例如：

```text
ttft_ms = 2812
source = "otel"
confidence = exact
```

或：

```text
visible_tokens = 842
source = "local_tokenizer"
confidence = estimated
```

UI 对估算数据显示：

```text
~842
```

而不是装作精确数字。

---

# 38. Pricing Engine

不要联网实时抓官方价格作为唯一方式。

使用：

```text
versioned pricing catalog
```

例如：

```json
{
  "model": "gpt-5.6-sol",
  "provider": "openai",
  "effective_from": "2026-08-01",
  "short_context": {
    "input": 5.0,
    "cached_input": 0.5,
    "cache_write": 6.25,
    "output": 30.0
  }
}
```

价格仅作为示例数据结构。

运行时允许：

```text
codex-meter pricing list
codex-meter pricing update
codex-meter pricing override
```

历史 Session 必须使用：

```text
当时的 pricing snapshot
```

而不是新价格重新计算后把历史数据改掉。

---

# 39. 成本计算注意事项

原则：

```text
reasoning_tokens ⊂ output_tokens
```

不能重复计费。

缓存计费根据 provider/model 的 pricing rules 计算。

对于支持独立 cache-write 的 provider：

```text
Input
├── cached read
├── cache write
└── regular input
```

Pricing Engine 应使用 provider-specific strategy，而不是在 UI 里面写死计算公式。

---

# 40. CLI Commands

建议：

```bash
codex-meter
```

启动 Dashboard。

```bash
codex-meter today
```

今日统计。

```bash
codex-meter live
```

实时监控。

```bash
codex-meter sessions
```

历史 Session。

```bash
codex-meter models
```

模型统计。

```bash
codex-meter cache
```

缓存统计。

```bash
codex-meter perf
```

性能统计。

```bash
codex-meter export
```

导出。

```bash
codex-meter doctor
```

检测当前 Codex 版本能够获取哪些数据。

---

# 41. Capability Detection

`doctor` 很重要。

输出：

```text
Codex Meter Doctor

Codex version              ✓ 0.xxx
Session JSONL              ✓
Reasoning usage            ✓
Cached input               ✓
Cache write                ✓
OpenTelemetry              ✓
App Server                 ✓
Raw response events        ✓ Experimental
WebSocket timings          ✓
Network probe              ○ Disabled
MITM diagnostic            ○ Disabled
```

不同 Codex 版本能力不一致时软件也不会崩。

---

# 42. Export

支持：

```text
JSON
JSONL
CSV
```

例如：

```bash
codex-meter export \
  --from 2026-08-01 \
  --to 2026-08-12 \
  --format csv
```

以及：

```bash
codex-meter export --session xxx
```

---

# 43. Privacy Mode

默认：

```text
store_prompt = false
store_response = false
store_tool_output = false
store_headers = false
```

只存：

```text
token counts
model
effort
timings
tool type
status
cost
project metadata
```

用户主动启用：

```text
diagnostic_payload_logging = true
```

才保存 payload。

---

# 44. 性能要求

统计工具自身不能拖慢 Codex。

要求：

```text
Collector asynchronous
UI separate from hot path
SQLite batch writes
WAL mode
bounded channel
drop low-priority telemetry when overloaded
```

目标：

```text
正常模式额外 CPU < 2%
内存 < 100 MB
请求额外延迟趋近 0
```

统计模块出现异常：

```text
必须 fail-open
```

即：

> Meter 挂了，Codex 仍然正常工作。

---

# 45. 数据保留

默认：

```text
raw events       7 days
call metrics     90 days
aggregates       unlimited
```

允许：

```text
retention.raw_days
retention.call_days
```

配置。

避免 JSONL / telemetry 长期积累占大量磁盘。

---

# 46. MVP 范围

第一版先不要做太大。

## V0.1

必须完成：

```text
SQLite
Session JSONL parser
model
reasoning effort
input tokens
cached input
output tokens
reasoning tokens
total tokens
API-equivalent USD
按日统计
按模型统计
按 reasoning effort 统计
蓝色 TUI
```

---

## V0.2

增加：

```text
OTel collector
Turn E2E
TTFT
TTFM
Tool duration
TPS
P50/P95
```

---

## V0.3

增加：

```text
App Server integration
per LLM call usage
rawResponse/completed
retry analysis
multi-call turn waterfall
```

---

## V0.4

增加：

```text
Cache Write
Cache Savings
Cache Reuse Efficiency
Context Amplification
Retry Tax
Compaction analysis
Project analytics
```

---

## V0.5

增加：

```text
Codex TUI /meter overlay
Footer live metrics
```

---

## V1.0

再考虑：

```text
local proxy
network diagnostics
provider adapters
MITM diagnostic
multi-agent cost attribution
```

---

# 47. 第一阶段工程策略

**不要先改 Codex 网络栈。**

先实现：

```text
JSONL
+
OTel
+
App Server
```

这三个数据源。

因为目前官方 Codex 已经暴露了大量你需要的数据：

* token usage
* reasoning token
* cached input
* request duration
* inference time
* TTFT
* TBT
* Turn E2E
* Tool duration

等数据模型和 Dashboard 稳定之后，再决定哪些数据必须通过 source instrumentation 或 proxy 获得。

---

# 48. 最终目标

最终 Codex CLI 底部应该可以长期看到：

```text
GPT-5.6 medium │ 78% cache │ $0.31 eq │ TTFT 2.7s │ 68 tok/s
```

按：

```text
/meter
```

打开：

```text
Token
Cost
Cache
Reasoning
Latency
TPS
Tool
Session
Project
```

完整 Dashboard。

这个工具最终应该让用户可以非常直观地回答：

> 我的 Codex 到底把 Token 花在哪里了？

以及：

> 同一模型到底哪个 reasoning level 性价比最高？

以及：

> 这次慢，到底慢在模型、thinking、网络，还是 Tool？

以及：

> 为什么我的 Token 突然暴涨？

---

# 49. 给 Codex 的实现要求

请按照以下优先级直接开始工程实现：

1. 检查当前安装的 Codex CLI 版本以及 openai/codex 当前源码结构。
2. 找出 TokenUsage、Turn、Thread、reasoning effort、OTel、App Server、session rollout JSONL 对应源码。
3. 不要凭字段名猜测语义，针对当前版本验证真实 event schema。
4. 先建立统一 `MeterEvent` 数据模型。
5. 创建 SQLite schema 和 migration。
6. 实现 Session JSONL Collector。
7. 实现历史 Token 聚合。
8. 实现 Model × Reasoning Effort 聚合。
9. 实现 Pricing Engine。
10. 实现深蓝色 TUI Overview。
11. 再接 OTel 实时数据。
12. 实现 TTFT / TTFM / E2E / Tool Time。
13. 再接 App Server，实现 per-response / per-LLM-call 统计。
14. 最后才修改 Codex TUI，加入 `/meter` Overlay 与 Footer。
15. 不要为了方便把业务逻辑写进 TUI。
16. 所有 experimental Codex API 必须使用 Adapter 和 capability detection。
17. 遇到缺失指标允许显示 Unknown / N/A，禁止伪造。
18. 默认不得记录用户 Prompt、API Key、Authorization、Cookie 或完整模型回复。
19. Meter 任意组件异常都不得影响 Codex 主流程。
20. 所有统计均增加 automated tests，尤其测试 cumulative token event 不得重复计数。

第一阶段完成后，给出：

```text
目录结构
数据流图
SQLite schema
事件模型
Collector 实现
Pricing Engine
TUI Screenshot / Demo
运行方法
测试结果
目前能采到与采不到的指标
下一阶段 TODO
```

不要只生成设计文档，直接建立可运行项目并完成 MVP。
