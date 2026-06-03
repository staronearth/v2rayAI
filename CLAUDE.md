# CLAUDE.md

本文件为 Claude Code 在本仓库工作时的指引。

## 项目概述

v2rayAI 是一个 **AI 辅助的 V2Ray/Xray 代理配置桌面应用**。用户用自然语言描述需求,AI 生成/诊断 Xray 配置、导入订阅、解析分享链接。

- **技术栈**:Tauri 2 + Rust(后端) + React 19 + Vite(前端)
- **包名**:`v2rayai`,版本 `0.1.0`

## 目录结构

```
src/                      前端 (React)
  App.jsx                 路由与全局状态
  pages/ChatPage.jsx      AI 对话页(AI 功能前端入口)
  pages/{Servers,Settings,Tools,Logs,Traffic}Page.jsx
  components/             Sidebar / StatusBar
src-tauri/src/            后端 (Rust)
  lib.rs                  Tauri 命令层 + 上下文组装 + AppState
  types.rs                共享类型:Role enum + ChatMessage
  ai_service.rs           OpenAI 兼容 HTTP 客户端(瘦封装)
  agent.rs                harness 式 agentic loop + 工具注册表 + 系统提示词
  knowledge_base.rs       文档 RAG(内置 Xray 文档分块 + TF-IDF)
  chat_history.rs         会话持久化 + 历史 RAG
  config_generator.rs     ServerConfig 类型 + 分享链接/订阅解析 + Xray 配置生成
  core_manager.rs         Xray 内核下载/启动/管理
  sub_converter.rs        subconverter 集成
  config_manager / health_monitor / sys_proxy / traffic_monitor / secure_store
```

## 常用命令

```bash
# 前端开发服务器
npm run dev
# Tauri 桌面应用(开发模式,前后端一起)
npm run tauri dev
# 构建发布版
npm run tauri build

# Rust 编译检查(后端)
cd src-tauri && cargo check
# Rust 单元测试
cd src-tauri && cargo test --lib
```

> 注意:`src-tauri/` 是 Rust crate 根(`Cargo.toml` 在此),cargo 命令需在该目录下执行。

## AI 流程(核心)

一次对话的数据流:

```
ChatPage.jsx
  → invoke('chat_with_ai', { message, history, settings, context })
lib.rs::chat_with_ai
  → 双路 RAG 并发检索:文档 RAG(knowledge_base) + 历史 RAG(chat_history)
  → build_system_message = 系统提示词 + RAG + 本地环境快照
  → thread = [system] → [history…] → [user]
agent.rs::run  (agentic loop,最多 MAX_ITERATIONS=5 轮)
  → ai_service.complete(thread) → 模型回复
  → 解析工具调用 → allowlist 校验 → 执行 → 结果回灌 thread → 重复
  → 无工具调用时返回 AgentResult { message, parsed_servers }
ai_service.rs::complete
  → POST {base_url}/chat/completions(OpenAI 兼容)
```

**关键约定**:
- 必须保持 **OpenAI 兼容端点**(支持第三方中转 / 本地模型),不绑定特定厂商。
- 工具白名单见 `agent.rs::ALLOWED_TOOLS`,不可让模型调用白名单外工具。
- 前端动作协议 `[[APP_ACTION:...]]` 只触发前端二次确认,不代表已执行。

## 开发习惯与约定

- **先写测试,再写业务(TDD)**:实现任何后端逻辑前,先在对应模块的 `#[cfg(test)] mod tests` 中写测试用例描述预期行为,再写实现让测试通过。参考 `agent.rs`、`config_generator.rs` 已有的测试风格。
- **每次改动后必须** `cargo check` + `cargo test --lib` 通过,再视为完成。
- **重构按阶段推进**:每阶段独立编译、独立测试、独立提交,保持每步可回滚。
- **回复用中文**,技术术语保留英文。
- 提交信息说明「做了什么、为什么」;未经明确要求不要 push。

## 当前进行中的重构

AI 流程正在重构,目标:native function calling + 流式输出 + RAG 升级 + 架构解耦(保持 OpenAI 兼容,native tools 带自动探测回退)。
- ✅ 阶段 0:类型解耦(新增 `types.rs`,统一 `ChatMessage` + `Role` enum,`Tool` 变体已预留)
- ⏳ 阶段 1:`ai_service` 加 tools/tool_calls 解析 + SSE 流式 + 能力探测
- ⏳ 阶段 2:`agent.rs` 双模式循环(native / 文本协议回退)+ 工具 JSON Schema
- ⏳ 阶段 3:RAG 升级(embedding 向量 + 中文分词 + 历史检索索引化)
- ⏳ 阶段 4:前端流式打字机渲染
