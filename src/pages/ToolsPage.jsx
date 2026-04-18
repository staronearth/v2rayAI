import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

// ── Tool registry: each tool defined as metadata ────────────────────────────
const BUILTIN_TOOLS = [
  {
    id: 'subconverter',
    name: '订阅转换',
    icon: '🔄',
    desc: '将 Clash YAML / Surge / SIP002 等格式自动转换为 v2ray 可用格式',
    tags: ['订阅', '转换', '格式'],
  },
  {
    id: 'latency-test',
    name: '批量测速',
    icon: '⚡',
    desc: '对所有节点进行 TCP 延迟测试，快速筛出高速节点',
    tags: ['延迟', '速度'],
    comingSoon: true,
  },
  {
    id: 'config-export',
    name: '配置导出',
    icon: '📤',
    desc: '导出 Xray/V2ray JSON 配置文件，方便在其他客户端使用',
    tags: ['配置', '导出'],
    comingSoon: true,
  },
  {
    id: 'ip-checker',
    name: 'IP 查询',
    icon: '🌍',
    desc: '查询当前出口 IP 及地理位置，验证代理是否生效',
    tags: ['IP', '检测'],
    comingSoon: true,
  },
]

export default function ToolsPage() {
  const [activeTool, setActiveTool] = useState(null) // null = grid view, toolId = detail view
  const [hubUrl, setHubUrl] = useState('')

  return (
    <div className="chat-container">
      <div className="page-header">
        <div>
          <div className="page-title">🔧 工具箱</div>
          <div className="page-subtitle">订阅转换、格式处理等实用工具</div>
        </div>
        {activeTool && (
          <button className="btn btn-secondary btn-sm" onClick={() => setActiveTool(null)}>
            ← 返回工具列表
          </button>
        )}
      </div>

      <div className="settings-container page-enter">
        {!activeTool ? (
          /* ════════════════ Grid View ════════════════ */
          <>
            {/* Tools Hub URL */}
            <div className="settings-section">
              <div className="settings-section-title">🌐 工具仓库</div>
              <div className="settings-card" style={{ padding: 'var(--space-sm) var(--space-md)' }}>
                <div style={{ display: 'flex', gap: 'var(--space-sm)', alignItems: 'center' }}>
                  <input
                    className="settings-input"
                    placeholder="填入 Tools Hub 地址以获取更多社区工具..."
                    value={hubUrl}
                    onChange={e => setHubUrl(e.target.value)}
                    style={{ flex: 1 }}
                    id="tools-hub-url"
                  />
                  <button className="btn btn-secondary btn-sm" disabled={!hubUrl.trim()}
                    onClick={() => { /* TODO: fetch tool registry from hub */ }}
                    id="tools-hub-fetch-btn"
                  >
                    🔍 获取
                  </button>
                </div>
                <div style={{ fontSize: '0.75rem', color: 'var(--text-muted)', marginTop: 6 }}>
                  从社区 Hub 获取第三方工具插件，扩展工具箱功能
                </div>
              </div>
            </div>

            {/* Tool Cards Grid */}
            <div className="settings-section">
              <div className="settings-section-title">📦 已安装工具</div>
              <div style={{
                display: 'grid',
                gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))',
                gap: 'var(--space-md)',
              }}>
                {BUILTIN_TOOLS.map(tool => (
                  <ToolCard
                    key={tool.id}
                    tool={tool}
                    onClick={() => !tool.comingSoon && setActiveTool(tool.id)}
                  />
                ))}
              </div>
            </div>
          </>
        ) : (
          /* ════════════════ Detail View ════════════════ */
          <>
            {activeTool === 'subconverter' && <SubConverterDetail />}
          </>
        )}
      </div>
    </div>
  )
}

// ══════════════════════════════════════════════════════════════════════════════
// Tool Card (compact grid item)
// ══════════════════════════════════════════════════════════════════════════════

function ToolCard({ tool, onClick }) {
  return (
    <div
      onClick={onClick}
      style={{
        background: 'var(--card-bg)',
        border: '1px solid var(--border)',
        borderRadius: 'var(--radius-md)',
        padding: 'var(--space-md)',
        cursor: tool.comingSoon ? 'default' : 'pointer',
        transition: 'all 0.2s ease',
        position: 'relative',
        opacity: tool.comingSoon ? 0.5 : 1,
      }}
      className="tool-card"
      id={`tool-card-${tool.id}`}
    >
      <div style={{
        fontSize: '2rem',
        marginBottom: 'var(--space-sm)',
        filter: tool.comingSoon ? 'grayscale(1)' : 'none',
      }}>
        {tool.icon}
      </div>
      <div style={{ fontWeight: 600, fontSize: 'var(--font-size-md)', marginBottom: 4 }}>
        {tool.name}
      </div>
      <div style={{
        fontSize: 'var(--font-size-sm)',
        color: 'var(--text-secondary)',
        lineHeight: 1.5,
        display: '-webkit-box',
        WebkitLineClamp: 2,
        WebkitBoxOrient: 'vertical',
        overflow: 'hidden',
      }}>
        {tool.desc}
      </div>
      <div style={{ display: 'flex', gap: 4, marginTop: 'var(--space-sm)', flexWrap: 'wrap' }}>
        {tool.tags.map(tag => (
          <span key={tag} style={{
            fontSize: '0.7rem',
            padding: '1px 8px',
            borderRadius: 10,
            background: 'rgba(99,102,241,0.1)',
            color: 'var(--accent-light)',
            border: '1px solid rgba(99,102,241,0.2)',
          }}>
            {tag}
          </span>
        ))}
      </div>
      {tool.comingSoon && (
        <div style={{
          position: 'absolute', top: 8, right: 8,
          fontSize: '0.7rem', padding: '2px 8px',
          borderRadius: 10, background: 'rgba(251,191,36,0.15)',
          color: '#fbbf24', border: '1px solid rgba(251,191,36,0.3)',
        }}>
          即将推出
        </div>
      )}
    </div>
  )
}

// ══════════════════════════════════════════════════════════════════════════════
// SubConverter Detail Page
// ══════════════════════════════════════════════════════════════════════════════

function SubConverterDetail() {
  const [scStatus, setScStatus]   = useState(null)
  const [scLoading, setScLoading] = useState('')
  const [scError, setScError]     = useState('')

  const [progressLogs, setProgressLogs] = useState([])
  const [downloadPercent, setDownloadPercent] = useState(-1)
  const logEndRef = useRef(null)

  const [testUrl, setTestUrl]       = useState('')
  const [testResult, setTestResult] = useState(null)
  const [testing, setTesting]       = useState(false)

  // Auto-detect on mount + listen to progress events
  useEffect(() => {
    (async () => {
      try { setScStatus(await invoke('get_subconverter_status')) } catch {}
    })()

    const unlisten = listen('sc-progress', (event) => {
      const { stage, message, percent } = event.payload
      const ts = new Date().toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' })

      if (stage === 'download') {
        setDownloadPercent(percent)
        if (percent === 0 || percent % 25 === 0 || percent === 100) {
          setProgressLogs(prev => [...prev, { ts, message, stage }])
        } else {
          setProgressLogs(prev => {
            const copy = [...prev]
            if (copy.length > 0 && copy[copy.length - 1].stage === 'download') {
              copy[copy.length - 1] = { ts, message, stage }
            } else {
              copy.push({ ts, message, stage })
            }
            return copy
          })
        }
      } else {
        setDownloadPercent(stage === 'done' ? -1 : downloadPercent)
        setProgressLogs(prev => [...prev, { ts, message, stage }])
      }
    })

    return () => { unlisten.then(fn => fn()) }
  }, [])

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [progressLogs])

  const handleInstall = async () => {
    setScLoading('安装')
    setScError('')
    setProgressLogs([])
    setDownloadPercent(0)
    try {
      const path = await invoke('install_subconverter')
      setScStatus(prev => ({ ...prev, installed: true, path }))
      setProgressLogs(prev => [...prev, {
        ts: new Date().toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
        message: '正在启动服务...', stage: 'info'
      }])
      try {
        await invoke('start_subconverter')
        setScStatus(prev => ({ ...prev, running: true }))
        setProgressLogs(prev => [...prev, {
          ts: new Date().toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
          message: '🟢 服务已启动！', stage: 'done'
        }])
      } catch (e) { setScError(`安装成功但启动失败：${e}`) }
    } catch (e) { setScError(String(e)) }
    setScLoading('')
    setDownloadPercent(-1)
  }

  const handleTestConvert = async () => {
    if (!testUrl.trim()) return
    setTesting(true)
    setTestResult(null)
    try {
      const servers = await invoke('convert_subscription_via_tool', { url: testUrl.trim() })
      setTestResult({ ok: true, count: servers.length, servers })
    } catch (err) {
      setTestResult({ ok: false, error: `${err}` })
    }
    setTesting(false)
  }

  return (
    <>
      {/* ── SubConverter Main ── */}
      <div className="settings-section">
        <div className="settings-section-title">🔄 订阅转换工具 (subconverter)</div>
        <div className="settings-card">
          <p style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-secondary)', lineHeight: 1.7, margin: '0 0 var(--space-md)' }}>
            <strong>subconverter</strong> 能将 Clash YAML / Surge / SIP002 等格式的订阅自动转换为 v2ray 可用格式。
            <br />安装后在本地运行微服务 (<code>127.0.0.1:25500</code>)，导入订阅时会自动调用。
          </p>

          {/* Status badges */}
          {scStatus && (
            <div style={{ display: 'flex', gap: 'var(--space-sm)', flexWrap: 'wrap', marginBottom: 'var(--space-md)', alignItems: 'center' }}>
              <StatusBadge ok={scStatus.installed} labelOk="✅ 已安装" labelNo="❌ 未安装" colorOk="34,197,94" colorNo="239,68,68" />
              {scStatus.installed && (
                <StatusBadge ok={scStatus.running} labelOk="🟢 运行中" labelNo="⏹ 未运行" colorOk="6,182,212" colorNo="100,116,139" />
              )}
              {scStatus.path && (
                <span style={{
                  fontSize: '0.75rem', color: 'var(--text-muted)',
                  overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 320,
                }} title={scStatus.path}>
                  📂 {scStatus.path}
                </span>
              )}
            </div>
          )}

          {scError && (
            <div style={{ color: 'var(--error)', fontSize: 'var(--font-size-sm)', marginBottom: 'var(--space-md)', whiteSpace: 'pre-wrap' }}>
              ⚠️ {scError}
            </div>
          )}

          {/* Action buttons */}
          <div style={{ display: 'flex', gap: 'var(--space-sm)', flexWrap: 'wrap' }}>
            <button className="btn btn-secondary btn-sm"
              onClick={async () => {
                setScLoading('检测'); setScError('')
                try { setScStatus(await invoke('get_subconverter_status')) }
                catch (e) { setScError(String(e)) }
                setScLoading('')
              }}
              disabled={!!scLoading}
            >
              {scLoading === '检测' ? '⏳ 检测中...' : '🔍 检测状态'}
            </button>

            <button className="btn btn-primary btn-sm" onClick={handleInstall} disabled={!!scLoading}>
              {scLoading === '安装' ? '⏳ 安装中...' : '⬇️ 一键安装'}
            </button>

            {scStatus?.installed && !scStatus?.running && (
              <button className="btn btn-secondary btn-sm"
                onClick={async () => {
                  setScLoading('启动'); setScError('')
                  try {
                    await invoke('start_subconverter')
                    setScStatus(prev => ({ ...prev, running: true }))
                  } catch (e) { setScError(String(e)) }
                  setScLoading('')
                }}
                disabled={!!scLoading}
              >
                {scLoading === '启动' ? '⏳' : '▶️ 启动服务'}
              </button>
            )}

            {scStatus?.running && (
              <button className="btn btn-secondary btn-sm"
                onClick={async () => {
                  setScLoading('停止'); setScError('')
                  try {
                    await invoke('stop_subconverter')
                    setScStatus(prev => ({ ...prev, running: false }))
                  } catch (e) { setScError(String(e)) }
                  setScLoading('')
                }}
                disabled={!!scLoading}
              >
                {scLoading === '停止' ? '⏳' : '⏹ 停止服务'}
              </button>
            )}

            <a href="https://github.com/tindy2013/subconverter" target="_blank" rel="noreferrer"
              className="btn btn-secondary btn-sm" style={{ textDecoration: 'none' }}>
              📦 GitHub
            </a>
          </div>

          {/* Progress bar */}
          {downloadPercent >= 0 && (
            <div style={{ marginTop: 'var(--space-md)' }}>
              <div style={{ height: 6, background: 'rgba(255,255,255,0.06)', borderRadius: 3, overflow: 'hidden' }}>
                <div style={{
                  height: '100%', width: `${downloadPercent}%`,
                  background: 'linear-gradient(90deg, var(--primary), var(--accent))',
                  borderRadius: 3, transition: 'width 0.3s ease',
                }} />
              </div>
              <div style={{ fontSize: '0.75rem', color: 'var(--text-muted)', marginTop: 4, textAlign: 'right' }}>
                {downloadPercent}%
              </div>
            </div>
          )}

          {/* Realtime log console */}
          {progressLogs.length > 0 && (
            <div style={{
              marginTop: 'var(--space-md)', background: 'rgba(0,0,0,0.3)',
              border: '1px solid var(--border)', borderRadius: 'var(--radius-sm)',
              padding: 'var(--space-sm)', maxHeight: 200, overflowY: 'auto',
              fontFamily: 'monospace', fontSize: '0.78rem', lineHeight: 1.7,
            }}>
              {progressLogs.map((log, i) => (
                <div key={i} style={{ color: stageColor(log.stage) }}>
                  <span style={{ color: 'var(--text-muted)', marginRight: 8 }}>{log.ts}</span>
                  {log.message}
                </div>
              ))}
              <div ref={logEndRef} />
            </div>
          )}
        </div>
      </div>

      {/* ── Test Conversion ── */}
      <div className="settings-section">
        <div className="settings-section-title">🧪 转换测试</div>
        <div className="settings-card">
          <p style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-secondary)', margin: '0 0 var(--space-sm)' }}>
            输入一个订阅 URL，测试 subconverter 是否能正确转换。
          </p>
          <div style={{ display: 'flex', gap: 'var(--space-sm)' }}>
            <input
              className="settings-input"
              placeholder="https://example.com/subscribe?token=xxx"
              value={testUrl}
              onChange={e => setTestUrl(e.target.value)}
              style={{ flex: 1 }}
              id="test-convert-input"
            />
            <button className="btn btn-primary btn-sm" onClick={handleTestConvert}
              disabled={testing || !scStatus?.running} id="test-convert-btn">
              {testing ? '⏳ 转换中...' : '🚀 测试转换'}
            </button>
          </div>
          {!scStatus?.running && (
            <div style={{ fontSize: '0.75rem', color: 'var(--text-muted)', marginTop: 'var(--space-xs)' }}>
              需要先启动 subconverter 服务才能测试转换
            </div>
          )}

          {testResult && (
            <div style={{
              marginTop: 'var(--space-md)',
              background: testResult.ok ? 'rgba(34,197,94,0.08)' : 'rgba(239,68,68,0.08)',
              border: `1px solid ${testResult.ok ? 'rgba(34,197,94,0.2)' : 'rgba(239,68,68,0.2)'}`,
              borderRadius: 'var(--radius-sm)', padding: 'var(--space-md)',
            }}>
              {testResult.ok ? (
                <>
                  <div style={{ fontWeight: 600, marginBottom: 'var(--space-sm)', color: '#4ade80' }}>
                    ✅ 转换成功！共 {testResult.count} 个节点
                  </div>
                  <div style={{
                    maxHeight: 200, overflowY: 'auto',
                    fontSize: '0.8rem', fontFamily: 'monospace', color: 'var(--text-secondary)', lineHeight: 1.6,
                  }}>
                    {testResult.servers.slice(0, 20).map((s, i) => (
                      <div key={i} style={{ padding: '2px 0', borderBottom: '1px solid var(--border)' }}>
                        <span style={{ color: 'var(--primary)', fontWeight: 500 }}>{s.protocol?.toUpperCase()}</span>
                        {' '}{s.name} — <span style={{ color: 'var(--text-muted)' }}>{s.address}:{s.port}</span>
                      </div>
                    ))}
                    {testResult.count > 20 && (
                      <div style={{ padding: '4px 0', color: 'var(--text-muted)' }}>
                        ...还有 {testResult.count - 20} 个节点
                      </div>
                    )}
                  </div>
                </>
              ) : (
                <div style={{ color: '#f87171' }}>
                  ❌ 转换失败：{testResult.error}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </>
  )
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function stageColor(stage) {
  switch (stage) {
    case 'download': return '#22d3ee'
    case 'done':     return '#4ade80'
    case 'error':    return '#f87171'
    default:         return 'var(--text-secondary)'
  }
}

function StatusBadge({ ok, labelOk, labelNo, colorOk, colorNo }) {
  const c = ok ? colorOk : colorNo
  return (
    <span style={{
      display: 'inline-flex', alignItems: 'center', gap: 6,
      background: `rgba(${c},0.12)`,
      border: `1px solid rgba(${c},0.3)`,
      borderRadius: 'var(--radius-sm)', padding: '4px 12px',
      fontSize: 'var(--font-size-sm)',
      color: `rgba(${c},1)`,
    }}>
      {ok ? labelOk : labelNo}
    </span>
  )
}
