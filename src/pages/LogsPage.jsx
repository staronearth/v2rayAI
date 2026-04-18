import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'

const LEVEL_COLORS = {
  ERROR: { bg: 'rgba(239,68,68,0.12)', text: '#f87171', border: 'rgba(239,68,68,0.25)' },
  WARN:  { bg: 'rgba(251,191,36,0.10)', text: '#fbbf24', border: 'rgba(251,191,36,0.25)' },
  INFO:  { bg: 'rgba(99,102,241,0.08)', text: '#818cf8', border: 'rgba(99,102,241,0.2)' },
  DEBUG: { bg: 'rgba(156,163,175,0.06)', text: '#9ca3af', border: 'rgba(156,163,175,0.15)' },
}

function formatTs(ts) {
  if (!ts) return ''
  const d = new Date(ts)
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 })
}

export default function LogsPage({ settings }) {
  const [appLogs, setAppLogs] = useState([])
  const [coreLogs, setCoreLogs] = useState([])
  const [activeTab, setActiveTab] = useState('app') // 'app' | 'core'
  const [levelFilter, setLevelFilter] = useState('ALL')
  const [autoRefresh, setAutoRefresh] = useState(true)
  const [searchQuery, setSearchQuery] = useState('')
  const logsEndRef = useRef(null)
  const intervalRef = useRef(null)

  // Fetch logs
  const fetchLogs = async () => {
    try {
      if (activeTab === 'app') {
        const logs = await invoke('get_app_logs', {
          count: 200,
          levelFilter: levelFilter === 'ALL' ? null : levelFilter,
        })
        setAppLogs(logs)
      } else {
        const logs = await invoke('get_core_logs', { count: 200 })
        setCoreLogs(logs)
      }
    } catch (e) {
      console.warn('Failed to fetch logs:', e)
    }
  }

  // Auto-refresh
  useEffect(() => {
    fetchLogs()
    if (autoRefresh) {
      intervalRef.current = setInterval(fetchLogs, 2000)
    }
    return () => { if (intervalRef.current) clearInterval(intervalRef.current) }
  }, [activeTab, levelFilter, autoRefresh])

  // Auto-scroll
  useEffect(() => {
    if (autoRefresh) {
      logsEndRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [appLogs, coreLogs])

  const handleClear = async () => {
    try {
      await invoke('clear_app_logs')
      setAppLogs([])
    } catch (e) { console.warn(e) }
  }

  const handleExport = () => {
    const logs = activeTab === 'app' ? appLogs : coreLogs
    const text = activeTab === 'app'
      ? logs.map(l => `[${formatTs(l.timestamp)}] [${l.level}] [${l.target}] ${l.message}`).join('\n')
      : logs.join('\n')
    const blob = new Blob([text], { type: 'text/plain' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `v2rayai-${activeTab}-logs-${Date.now()}.txt`
    a.click()
    URL.revokeObjectURL(url)
  }

  // Filtered display
  const filteredAppLogs = searchQuery
    ? appLogs.filter(l => l.message.toLowerCase().includes(searchQuery.toLowerCase()) || l.target.toLowerCase().includes(searchQuery.toLowerCase()))
    : appLogs

  const filteredCoreLogs = searchQuery
    ? coreLogs.filter(l => l.toLowerCase().includes(searchQuery.toLowerCase()))
    : coreLogs

  return (
    <div className="chat-container" style={{ display: 'flex', flexDirection: 'column' }}>
      <div className="page-header">
        <div>
          <div className="page-title">📋 日志</div>
          <div className="page-subtitle">应用运行日志与内核输出</div>
        </div>
        <div style={{ display: 'flex', gap: 'var(--space-sm)', alignItems: 'center' }}>
          <button className="btn btn-secondary btn-sm" onClick={handleExport} id="export-logs-btn">📥 导出</button>
          <button className="btn btn-secondary btn-sm" onClick={handleClear} id="clear-logs-btn">🗑️ 清空</button>
        </div>
      </div>

      {/* Tab bar + filters */}
      <div style={{
        display: 'flex', gap: 'var(--space-sm)', alignItems: 'center', padding: '0 var(--space-lg)',
        marginBottom: 'var(--space-sm)', flexWrap: 'wrap',
      }}>
        {/* Tabs */}
        <div style={{
          display: 'flex', background: 'rgba(255,255,255,0.04)', borderRadius: 'var(--radius-sm)',
          border: '1px solid rgba(255,255,255,0.06)', overflow: 'hidden',
        }}>
          {[
            { id: 'app', label: '⚡ 应用日志' },
            { id: 'core', label: '🔧 内核日志' },
          ].map(tab => (
            <button
              key={tab.id}
              onClick={() => { setActiveTab(tab.id); setSearchQuery('') }}
              id={`log-tab-${tab.id}`}
              style={{
                padding: '6px 14px', border: 'none', cursor: 'pointer',
                background: activeTab === tab.id ? 'rgba(99,102,241,0.2)' : 'transparent',
                color: activeTab === tab.id ? 'var(--accent-light)' : 'var(--text-secondary)',
                fontWeight: activeTab === tab.id ? 600 : 400,
                fontSize: '0.8rem', transition: 'all 0.15s',
              }}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {/* Level filter (app only) */}
        {activeTab === 'app' && (
          <select
            value={levelFilter}
            onChange={e => setLevelFilter(e.target.value)}
            style={{
              background: 'rgba(255,255,255,0.05)', color: 'var(--text-secondary)',
              border: '1px solid rgba(255,255,255,0.1)', borderRadius: 'var(--radius-sm)',
              padding: '5px 8px', fontSize: '0.78rem', cursor: 'pointer',
            }}
          >
            <option value="ALL">全部级别</option>
            <option value="ERROR">❌ ERROR</option>
            <option value="WARN">⚠️ WARN</option>
            <option value="INFO">ℹ️ INFO</option>
          </select>
        )}

        {/* Search */}
        <input
          placeholder="🔍 搜索日志…"
          value={searchQuery}
          onChange={e => setSearchQuery(e.target.value)}
          style={{
            flex: 1, minWidth: 150,
            background: 'rgba(255,255,255,0.04)', color: 'var(--text-primary)',
            border: '1px solid rgba(255,255,255,0.08)', borderRadius: 'var(--radius-sm)',
            padding: '5px 10px', fontSize: '0.78rem',
          }}
        />

        {/* Auto refresh toggle */}
        <label style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: '0.78rem', color: 'var(--text-secondary)', cursor: 'pointer', userSelect: 'none' }}>
          <input
            type="checkbox"
            checked={autoRefresh}
            onChange={e => setAutoRefresh(e.target.checked)}
            style={{ accentColor: 'var(--accent)' }}
          />
          自动刷新
        </label>

        {/* Stats badge */}
        <div style={{
          background: 'rgba(99,102,241,0.1)', borderRadius: 'var(--radius-sm)',
          padding: '4px 10px', fontSize: '0.72rem', color: 'var(--accent-light)',
          border: '1px solid rgba(99,102,241,0.2)',
        }}>
          {activeTab === 'app' ? filteredAppLogs.length : filteredCoreLogs.length} 条
        </div>
      </div>

      {/* Log entries */}
      <div style={{
        flex: 1, overflow: 'auto', padding: '0 var(--space-lg) var(--space-md)',
        fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', monospace",
        fontSize: '0.76rem', lineHeight: 1.5,
      }}>
        {activeTab === 'app' ? (
          filteredAppLogs.length === 0 ? (
            <div style={{ textAlign: 'center', color: 'var(--text-muted)', padding: 40, fontSize: '0.85rem' }}>
              暂无日志。应用运行时的操作会自动记录在这里。
            </div>
          ) : filteredAppLogs.map((log, i) => {
            const colors = LEVEL_COLORS[log.level] || LEVEL_COLORS.INFO
            return (
              <div key={i} style={{
                display: 'flex', gap: 8, padding: '3px 0',
                borderBottom: '1px solid rgba(255,255,255,0.03)',
                alignItems: 'flex-start',
              }}>
                <span style={{ color: 'var(--text-muted)', flexShrink: 0, minWidth: 88 }}>
                  {formatTs(log.timestamp)}
                </span>
                <span style={{
                  display: 'inline-block', minWidth: 48, textAlign: 'center',
                  background: colors.bg, color: colors.text,
                  border: `1px solid ${colors.border}`,
                  borderRadius: 3, padding: '0 6px', fontSize: '0.68rem',
                  fontWeight: 700, flexShrink: 0,
                }}>
                  {log.level}
                </span>
                <span style={{ color: 'var(--text-tertiary)', flexShrink: 0, maxWidth: 140, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {log.target}
                </span>
                <span style={{ color: 'var(--text-primary)', wordBreak: 'break-all' }}>
                  {log.message}
                </span>
              </div>
            )
          })
        ) : (
          filteredCoreLogs.length === 0 ? (
            <div style={{ textAlign: 'center', color: 'var(--text-muted)', padding: 40, fontSize: '0.85rem' }}>
              内核未运行或暂无输出。启动 Xray 内核后日志将显示在此。
            </div>
          ) : filteredCoreLogs.map((line, i) => {
            const isErr = line.startsWith('[ERR]')
            return (
              <div key={i} style={{
                padding: '2px 0',
                borderBottom: '1px solid rgba(255,255,255,0.03)',
                color: isErr ? '#f87171' : 'var(--text-primary)',
                wordBreak: 'break-all',
              }}>
                {line}
              </div>
            )
          })
        )}
        <div ref={logsEndRef} />
      </div>
    </div>
  )
}
