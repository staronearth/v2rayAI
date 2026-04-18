import { useState, useRef, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'

// ── Helpers ──────────────────────────────────────────────────────────────────

function genId() {
  return `conv-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`
}

function formatTime(ts) {
  if (!ts) return ''
  const d = new Date(ts)
  const now = new Date()
  const diffDays = Math.floor((now - d) / 86400000)
  if (diffDays === 0) return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  if (diffDays === 1) return '昨天'
  if (diffDays < 7) return `${diffDays}天前`
  return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })
}

const QUICK_PROMPTS = [
  { icon: '🔧', text: '帮我配置一个 VLESS + REALITY 的节点' },
  { icon: '🌐', text: '帮我配置 VMess + WebSocket + TLS（支持 CDN）' },
  { icon: '🛡️', text: '配置路由规则：国内直连，国外走代理，去广告' },
  { icon: '⚡', text: '我连接了但是网速很慢，帮我诊断和优化配置' },
]

// ── Main Component ────────────────────────────────────────────────────────────

export default function ChatPage({ settings, onAddServers, activeServer, isConnected }) {
  // Current conversation
  const [convId, setConvId] = useState(genId())
  const [convTitle, setConvTitle] = useState('新对话')
  const [messages, setMessages] = useState([])
  const [inputValue, setInputValue] = useState('')
  const [isLoading, setIsLoading] = useState(false)

  // History panel
  const [historyList, setHistoryList] = useState([])
  const [searchQuery, setSearchQuery] = useState('')
  const [showHistory, setShowHistory] = useState(false)
  const [historyLoading, setHistoryLoading] = useState(false)

  const messagesEndRef = useRef(null)
  const textareaRef = useRef(null)
  const saveTimerRef = useRef(null)

  // ── Scroll ──────────────────────────────────────────────────────────────────
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  // ── Auto-resize textarea ───────────────────────────────────────────────────
  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto'
      textareaRef.current.style.height = Math.min(textareaRef.current.scrollHeight, 120) + 'px'
    }
  }, [inputValue])

  // ── Load history list on panel open ────────────────────────────────────────
  useEffect(() => {
    if (showHistory) loadHistoryList(searchQuery)
  }, [showHistory])

  const loadHistoryList = async (q = '') => {
    setHistoryLoading(true)
    try {
      const list = q
        ? await invoke('search_chats', { query: q })
        : await invoke('list_chats')
      setHistoryList(list)
    } catch (e) {
      console.warn('Failed to load history:', e)
    }
    setHistoryLoading(false)
  }

  // ── Auto-save conversation (debounced 2s) ───────────────────────────────────
  const autoSave = useCallback((msgs, id, title) => {
    if (msgs.length === 0) return
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current)
    saveTimerRef.current = setTimeout(async () => {
      try {
        const now = Date.now()
        await invoke('save_chat', {
          conv: {
            id,
            title: title || msgs.find(m => m.role === 'user')?.content?.slice(0, 30) || '新对话',
            messages: msgs.map(m => ({ role: m.role, content: m.content, timestamp: m.timestamp || now })),
            created_at: msgs[0]?.timestamp || now,
            updated_at: now,
            summary: null,
          },
        })
      } catch (e) {
        console.warn('Auto-save failed:', e)
      }
    }, 2000)
  }, [])

  // ── Send message ────────────────────────────────────────────────────────────
  const handleSend = async () => {
    const message = inputValue.trim()
    if (!message || isLoading) return

    const userMsg = { role: 'user', content: message, timestamp: Date.now() }
    const newMessages = [...messages, userMsg]
    setMessages(newMessages)
    setInputValue('')
    setIsLoading(true)

    // Auto-title from first message
    let title = convTitle
    if (messages.length === 0) {
      title = message.slice(0, 30) + (message.length > 30 ? '…' : '')
      setConvTitle(title)
    }

    try {
      const result = await invoke('chat_with_ai', {
        message,
        history: messages.map(m => ({ role: m.role, content: m.content })),
        settings: {
          base_url: settings.aiBaseUrl,
          api_key: settings.aiApiKey,
          model: settings.aiModel,
        },
        context: activeServer ? {
          server_name: activeServer.name,
          protocol: activeServer.protocol,
          is_connected: !!isConnected,
          latency_ms: activeServer.latency ? parseInt(activeServer.latency) : null,
          routing_mode: settings.routingMode || 'smart',
        } : null,
      })

      // result is { message: string, parsedServers: ServerConfig[] }
      const aiText = result.message
      const assistantMsg = { role: 'assistant', content: aiText, timestamp: Date.now() }
      const finalMessages = [...newMessages, assistantMsg]
      setMessages(finalMessages)
      autoSave(finalMessages, convId, title)

      // Auto-add servers parsed by tool execution
      if (result.parsedServers?.length > 0 && onAddServers) {
        const servers = result.parsedServers.map((s, i) => ({
          ...s,
          id: s.id || `tool-${Date.now()}-${i}`,
          latency: null,
          source: 'tool',
        }))
        onAddServers(servers)
      }

      // Also try to extract config from AI-generated JSON blocks
      tryExtractConfig(aiText)
    } catch (e) {
      const errMsg = { role: 'assistant', content: `❌ 出错了: ${e}`, timestamp: Date.now() }
      setMessages(prev => [...prev, errMsg])
    } finally {
      setIsLoading(false)
    }
  }

  // ── New conversation ────────────────────────────────────────────────────────
  const handleNewConv = () => {
    setConvId(genId())
    setConvTitle('新对话')
    setMessages([])
    setInputValue('')
  }

  // ── Load conversation from history ──────────────────────────────────────────
  const handleLoadConv = async (id) => {
    try {
      const conv = await invoke('load_chat', { id })
      setConvId(conv.id)
      setConvTitle(conv.title)
      setMessages(conv.messages.map(m => ({ ...m })))
      setShowHistory(false)
    } catch (e) {
      console.warn('Failed to load conversation:', e)
    }
  }

  // ── Delete conversation ─────────────────────────────────────────────────────
  const handleDeleteConv = async (e, id) => {
    e.stopPropagation()
    try {
      await invoke('delete_chat', { id })
      if (id === convId) handleNewConv()
      loadHistoryList(searchQuery)
    } catch (e) {
      console.warn('Delete failed:', e)
    }
  }

  // ── Apply AI-generated config ───────────────────────────────────────────────
  const tryExtractConfig = (text) => {
    const jsonMatches = text.match(/```json\n([\s\S]*?)```/g)
    if (!jsonMatches || !onAddServers) return
    jsonMatches.forEach(match => {
      try {
        const code = match.replace(/```json\n?/, '').replace(/```$/, '').trim()
        const config = JSON.parse(code)
        if (config.outbounds) {
          const proxies = config.outbounds.filter(
            o => o.protocol !== 'freedom' && o.protocol !== 'blackhole'
          )
          const servers = proxies.map((proxy, i) => ({
            id: `ai-${Date.now()}-${i}`,
            name: `AI 生成 - ${proxy.protocol?.toUpperCase()}`,
            protocol: proxy.protocol,
            address: proxy.settings?.vnext?.[0]?.address || proxy.settings?.servers?.[0]?.address || 'unknown',
            port: proxy.settings?.vnext?.[0]?.port || proxy.settings?.servers?.[0]?.port || 443,
            fullConfig: config,
            latency: null,
            source: 'ai',
          }))
          if (servers.length > 0) onAddServers(servers)
        }
      } catch (_) {}
    })
  }

  const handleKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSend() }
  }

  // ── Render ──────────────────────────────────────────────────────────────────
  return (
    <div style={{ display: 'flex', height: '100%', overflow: 'hidden' }}>

      {/* ── History Sidebar ── */}
      <div style={{
        width: showHistory ? 260 : 0,
        minWidth: showHistory ? 260 : 0,
        overflow: 'hidden',
        transition: 'all 0.25s cubic-bezier(0.4,0,0.2,1)',
        background: 'rgba(10,10,30,0.7)',
        borderRight: showHistory ? '1px solid rgba(255,255,255,0.06)' : 'none',
        display: 'flex',
        flexDirection: 'column',
      }}>
        {showHistory && (
          <>
            <div style={{ padding: '16px 12px 8px', flexShrink: 0 }}>
              <div style={{ fontWeight: 700, fontSize: '0.85rem', color: 'var(--text-secondary)', marginBottom: 8 }}>
                历史对话
              </div>
              <input
                className="settings-input"
                placeholder="🔍 搜索对话…"
                value={searchQuery}
                onChange={e => { setSearchQuery(e.target.value); loadHistoryList(e.target.value) }}
                style={{ fontSize: '0.8rem', padding: '6px 10px' }}
              />
              <button
                className="btn btn-primary btn-sm"
                onClick={handleNewConv}
                style={{ width: '100%', marginTop: 8, fontSize: '0.8rem' }}
                id="new-conv-btn"
              >
                ✏️ 新建对话
              </button>
            </div>

            <div style={{ flex: 1, overflowY: 'auto', padding: '0 6px 12px' }}>
              {historyLoading ? (
                <div style={{ textAlign: 'center', color: 'var(--text-muted)', fontSize: '0.8rem', padding: 16 }}>
                  加载中…
                </div>
              ) : historyList.length === 0 ? (
                <div style={{ textAlign: 'center', color: 'var(--text-muted)', fontSize: '0.8rem', padding: 16 }}>
                  {searchQuery ? '没有找到相关对话' : '暂无历史对话'}
                </div>
              ) : historyList.map(conv => (
                <div
                  key={conv.id}
                  onClick={() => handleLoadConv(conv.id)}
                  style={{
                    padding: '9px 10px',
                    borderRadius: 'var(--radius-sm)',
                    cursor: 'pointer',
                    marginBottom: 2,
                    background: conv.id === convId ? 'rgba(99,102,241,0.15)' : 'transparent',
                    border: conv.id === convId ? '1px solid rgba(99,102,241,0.3)' : '1px solid transparent',
                    transition: 'background 0.15s',
                    position: 'relative',
                  }}
                  onMouseEnter={e => { if (conv.id !== convId) e.currentTarget.style.background = 'rgba(255,255,255,0.04)'}}
                  onMouseLeave={e => { if (conv.id !== convId) e.currentTarget.style.background = 'transparent'}}
                >
                  <div style={{
                    fontSize: '0.8rem', fontWeight: 600,
                    color: conv.id === convId ? 'var(--accent-light)' : 'var(--text-primary)',
                    whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
                    paddingRight: 20,
                  }}>
                    {conv.title}
                  </div>
                  <div style={{
                    fontSize: '0.7rem', color: 'var(--text-muted)',
                    whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
                    marginTop: 2,
                  }}>
                    {conv.preview}
                  </div>
                  <div style={{ fontSize: '0.65rem', color: 'var(--text-muted)', marginTop: 2 }}>
                    {formatTime(conv.updated_at)} · {conv.message_count} 条
                  </div>
                  <button
                    onClick={e => handleDeleteConv(e, conv.id)}
                    style={{
                      position: 'absolute', top: 6, right: 6,
                      background: 'none', border: 'none', cursor: 'pointer',
                      color: 'var(--text-muted)', fontSize: '0.75rem', padding: '2px 4px',
                      borderRadius: 4, opacity: 0.6,
                    }}
                    onMouseEnter={e => e.currentTarget.style.color = 'var(--error)'}
                    onMouseLeave={e => e.currentTarget.style.color = 'var(--text-muted)'}
                    title="删除"
                  >🗑</button>
                </div>
              ))}
            </div>
          </>
        )}
      </div>

      {/* ── Main Chat Area ── */}
      <div className="chat-container" style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        <div className="page-header">
          <div style={{ minWidth: 0 }}>
            <div className="page-title" style={{
              whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 400,
            }}>
              {convTitle}
            </div>
            <div className="page-subtitle">AI 代理配置助手</div>
          </div>
          <div style={{ display: 'flex', gap: 'var(--space-sm)', flexShrink: 0, alignItems: 'center' }}>
            <button
              className={`btn btn-secondary btn-sm ${showHistory ? 'active' : ''}`}
              onClick={() => setShowHistory(v => !v)}
              title="查看历史记录"
              style={{ background: showHistory ? 'rgba(99,102,241,0.2)' : 'transparent', border: showHistory ? '1px solid rgba(99,102,241,0.4)' : '' }}
              id="toggle-history-btn"
            >
              <span style={{ marginRight: '4px' }}>{showHistory ? '📖' : '🕒'}</span>
              {showHistory ? '收起历史' : '历史对话'}
            </button>
            {messages.length > 0 && (
              <button className="btn btn-primary btn-sm" onClick={handleNewConv} id="new-chat-btn">
                ✏️ 新对话
              </button>
            )}
          </div>
        </div>

        <div className="chat-messages">
          {messages.length === 0 ? (
            <div className="chat-welcome">
              <div className="chat-welcome-icon">⚡</div>
              <h2>欢迎使用 v2rayAI</h2>
              <p>
                我是你的 AI 代理配置助手，具备完整的 Xray/v2ray 知识库。<br />
                告诉我你的需求，我来帮你生成配置，翻墙像喝水一样简单！
              </p>
              <div className="quick-prompts">
                {QUICK_PROMPTS.map((prompt, i) => (
                  <button
                    key={i}
                    className="quick-prompt"
                    onClick={() => setInputValue(prompt.text)}
                    id={`quick-prompt-${i}`}
                  >
                    <div className="quick-prompt-icon">{prompt.icon}</div>
                    {prompt.text}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            messages.map((msg, i) => <MessageBubble key={i} msg={msg} onApplyConfig={config => {
              if (onAddServers) {
                const proxies = config.outbounds?.filter(o => o.protocol !== 'freedom' && o.protocol !== 'blackhole') || []
                const servers = proxies.map((proxy, j) => ({
                  id: `ai-${Date.now()}-${j}`,
                  name: `AI 生成 - ${proxy.protocol?.toUpperCase()}`,
                  protocol: proxy.protocol,
                  address: proxy.settings?.vnext?.[0]?.address || proxy.settings?.servers?.[0]?.address || 'unknown',
                  port: proxy.settings?.vnext?.[0]?.port || proxy.settings?.servers?.[0]?.port || 443,
                  fullConfig: config,
                  latency: null,
                  source: 'ai',
                }))
                if (servers.length) onAddServers(servers)
              }
            }} />)
          )}

          {isLoading && (
            <div className="chat-message assistant">
              <div className="chat-avatar assistant">⚡</div>
              <div className="chat-bubble">
                <div className="typing-indicator">
                  <div className="typing-dot" />
                  <div className="typing-dot" />
                  <div className="typing-dot" />
                </div>
              </div>
            </div>
          )}
          <div ref={messagesEndRef} />
        </div>

        <div className="chat-input-area">
          <div className="chat-input-wrapper">
            <textarea
              ref={textareaRef}
              className="chat-input selectable"
              placeholder="描述你的需求，例如：我有一台 VPS，IP 是 1.2.3.4，想配置 VLESS + REALITY..."
              value={inputValue}
              onChange={e => setInputValue(e.target.value)}
              onKeyDown={handleKeyDown}
              rows={1}
              id="chat-input"
            />
            <button
              className="chat-send-btn"
              onClick={handleSend}
              disabled={!inputValue.trim() || isLoading}
              id="chat-send-btn"
            >
              ↑
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

// ── Message Bubble ────────────────────────────────────────────────────────────

function MessageBubble({ msg, onApplyConfig }) {
  const [copied, setCopied] = useState(false)

  const handleCopy = (text) => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  return (
    <div className={`chat-message ${msg.role}`}>
      <div className={`chat-avatar ${msg.role}`}>{msg.role === 'user' ? '👤' : '⚡'}</div>
      <div className="chat-bubble">
        <div className="chat-message-content selectable">
          {renderMarkdown(msg.content, onApplyConfig, handleCopy)}
        </div>
        {msg.timestamp && (
          <div style={{ fontSize: '0.65rem', color: 'var(--text-muted)', marginTop: 4, textAlign: msg.role === 'user' ? 'right' : 'left' }}>
            {formatTime(msg.timestamp)}
          </div>
        )}
      </div>
    </div>
  )
}

// ── Markdown Renderer ─────────────────────────────────────────────────────────

function renderMarkdown(text, onApplyConfig, onCopy) {
  const parts = text.split(/(```[\s\S]*?```)/g)
  return parts.map((part, i) => {
    if (part.startsWith('```')) {
      const langMatch = part.match(/```(\w*)\n?/)
      const lang = langMatch?.[1] || ''
      const code = part.replace(/```\w*\n?/, '').replace(/```$/, '').trim()

      let configObj = null
      if (lang === 'json') {
        try {
          const obj = JSON.parse(code)
          if (obj.outbounds || obj.inbounds || obj.routing || obj.protocol) configObj = obj
        } catch (_) {}
      }

      return (
        <div key={i} style={{ position: 'relative', margin: '8px 0' }}>
          {lang && (
            <div style={{
              fontSize: '0.7rem', color: 'var(--text-muted)',
              padding: '4px 12px 0', textTransform: 'uppercase', letterSpacing: '0.05em',
            }}>{lang}</div>
          )}
          <pre style={{ position: 'relative' }}>
            <code>{code}</code>
            <button
              onClick={() => onCopy?.(code)}
              style={{
                position: 'absolute', top: 8, right: 8, background: 'rgba(255,255,255,0.1)',
                border: 'none', borderRadius: 4, color: 'var(--text-secondary)',
                cursor: 'pointer', padding: '2px 8px', fontSize: '0.7rem',
              }}
            >
              复制
            </button>
          </pre>
          {configObj && onApplyConfig && (
            <button
              className="config-apply-btn"
              onClick={() => onApplyConfig(configObj)}
            >
              ✨ 应用此配置
            </button>
          )}
        </div>
      )
    }

    return (
      <span key={i}>
        {part.split('\n').map((line, j) => {
          if (line.startsWith('### ')) return <h3 key={j} style={{ margin: '8px 0 4px' }}>{line.slice(4)}</h3>
          if (line.startsWith('## ')) return <h2 key={j} style={{ margin: '10px 0 4px' }}>{line.slice(3)}</h2>
          if (line.startsWith('# ')) return <h1 key={j} style={{ margin: '12px 0 6px' }}>{line.slice(2)}</h1>
          if (line.startsWith('- ') || line.startsWith('* '))
            return <div key={j} style={{ paddingLeft: '1em' }}>• {processInline(line.slice(2))}</div>
          if (/^\d+\.\s/.test(line)) {
            const m = line.match(/^(\d+)\.\s(.*)/)
            return <div key={j} style={{ paddingLeft: '1em' }}>{m[1]}. {processInline(m[2])}</div>
          }
          if (line.trim() === '') return <br key={j} />
          return <p key={j} style={{ margin: '2px 0' }}>{processInline(line)}</p>
        })}
      </span>
    )
  })
}

function processInline(text) {
  const parts = text.split(/(\*\*.*?\*\*|`.*?`)/g)
  return parts.map((part, i) => {
    if (part.startsWith('**') && part.endsWith('**')) return <strong key={i}>{part.slice(2, -2)}</strong>
    if (part.startsWith('`') && part.endsWith('`')) return <code key={i}>{part.slice(1, -1)}</code>
    return part
  })
}
