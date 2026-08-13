# Codex Meter macOS 桌面版

Codex Meter Desktop 是给不想一直使用终端界面的用户准备的原生桌面应用。
它不会替换 Codex CLI，也不会修改 Codex Desktop；桌面版读取的仍然是
`codex-meter` 命令行版使用的同一套统计元数据。

## 可以看到什么

- 当前 Codex 与 Spark 的七日账号额度；
- Token、API 等价费用、缓存效率、调用次数与会话数；
- 可切日期的日、周、月、全部统计，以及日/周/月历史；
- 按项目和可选账号标签筛选；
- 缓存节省、重试、输出速度和只含元数据的 Network 洞察；
- CSV 导出、最近会话，以及本机与 SSH 远程源的增量刷新进度；
- 单独测试、刷新远程服务器，并显示状态、失败原因和安全取消。

第一次刷新会导入已有 Rollout 的统计元数据；以后只处理变化过的文件。
某台远程服务器连接失败时，应用会提示具体原因，但已经刷新的本机统计仍然可看。

## 数据与隐私

桌面版和命令行版共用 `~/.codex-meter/meter.db`。安装、打开、升级或删除
桌面应用都不会删除这个目录，所以原有 CLI 历史会自动出现在桌面版中。

Codex Meter 只保留用量和时间等元数据，不保存 Prompt、回答、推理内容、
Shell 命令、工具参数或输出、HTTP Header、Cookie、密钥或认证文件。
远程服务器同样使用 CLI 已有的“只传统计元数据”SSH 过滤流程。

## 怎么使用

1. 从“应用程序”打开 **Codex Meter**。
2. 在 Overview 顶部选择 Today、Week、Month 或 All time。
3. Project 默认是 **All projects**，也可以切换到某一个项目。
4. 点击右上角刷新按钮，导入本机和远程服务器的新统计。
5. History、Insights 和 Sessions 查看明细；Settings 管理账号标签、价格目录、数据路径和 SSH 别名。

快捷键：`⌘R` 刷新，`⌘,` 打开 Settings，`⌘1`/`⌘2`/`⌘3` 切换
Overview、History、Insights。日期左右按钮切换统计窗口；Export 只导出筛选后的统计元数据。

周额度会并行加载，不受日期和项目筛选影响。如果额度不可用，先确认终端里
`codex --version` 可以运行，再刷新。桌面版除了系统 PATH，还会安全查找
Homebrew、`~/.local/bin`、Volta、FNM、npm-global 和 NVM 的常见位置。

添加远程服务器之前，请先确认终端里 `ssh 别名` 能正常连接。然后在 Settings
输入同一个别名并点击 **Add server**。第一次元数据同步可能较久，界面会显示
进度；以后刷新是增量的。

## 安装公开预览版

当前支持 macOS 12 及以上版本，请下载与电脑相符的 DMG：

- [Apple Silicon（M1/M2/M3/M4）](https://github.com/DelicateNorman/codex-meter/releases/download/v0.17.0-beta.1/codex-meter-desktop-macos-arm64.dmg)
- [Intel](https://github.com/DelicateNorman/codex-meter/releases/download/v0.17.0-beta.1/codex-meter-desktop-macos-x86_64.dmg)

打开 DMG，把 Codex Meter 拖进“应用程序”。这个预览版尚未签名和公证，macOS
第一次打开时可能拦截。请右键点击 Codex Meter，选择“打开”；如果仍被拦截，
前往“系统设置 → 隐私与安全性”，找到 Codex Meter 后选择“仍要打开”。下载后
可以用 Release 页面中的 `SHA256SUMS` 核对 SHA-256。

开发和测试不要求加入付费 Apple 开发者计划。未来的通用正式版应使用
Developer ID 签名和 Apple 公证，从而去掉第一次打开时的额外步骤。

## 从源码运行

安装当前 Node.js LTS 和稳定版 Rust，然后运行：

```bash
git clone https://github.com/DelicateNorman/codex-meter.git
cd codex-meter/desktop
npm ci
npm run tauri dev
```

构建 release 应用和 DMG：

```bash
npm run tauri build
```

产物位于 `desktop/src-tauri/target/release/bundle/`。构建过程不需要移动或修改
`~/.codex-meter`。
