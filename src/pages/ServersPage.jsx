import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

// ── helpers ───────────────────────────────────────────────────────────────────

function genId() {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function getLatencyClass(latency) {
  if (!latency || latency === 'testing...') return ''
  const ms = parseInt(latency)
  if (ms < 100) return 'fast'
  if (ms < 200) return 'medium'
  return 'slow'
}

function fmtTime(ts) {
  if (!ts) return '从未'
  return new Date(ts).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

// Protocol-specific default fields
const PROTOCOL_DEFAULTS = {
  vless:       { encryption: 'none', flow: '', network: 'tcp', security: 'tls', sni: '', fingerprint: 'chrome', path: '', host: '', uuid: '', realityPublicKey: '', realityShortId: '' },
  vmess:       { alterId: 0, encryption: 'auto', network: 'tcp', security: 'none', sni: '', path: '', host: '', uuid: '' },
  trojan:      { password: '', network: 'tcp', security: 'tls', sni: '', fingerprint: 'chrome', path: '' },
  shadowsocks: { password: '', encryption: 'aes-256-gcm' },
}

// ── Main Page ──────────────────────────────────────────────────────────────────

export default function ServersPage({ servers, setServers, subscriptions, setSubscriptions, activeServer, onConnect, onDisconnect, onNavigate }) {
  const [tab, setTab] = useState('nodes')
  const [showAddModal,  setShowAddModal]  = useState(false)
  const [showSubModal,  setShowSubModal]  = useState(false)
  const [showLinkModal, setShowLinkModal] = useState(false)
  const [subUrl,  setSubUrl]  = useState('')
  const [subName, setSubName] = useState('')
  const [proxyLink, setProxyLink] = useState('')
  const [importing,   setImporting]   = useState(false)
  const [importError, setImportError] = useState('')
  const [updateError, setUpdateError] = useState('')
  const [updatingId,  setUpdatingId]  = useState(null)
  const [filterSub,   setFilterSub]   = useState('all')
  const [confirmDelete, setConfirmDelete] = useState(null) // sub to confirm delete

  const displayedServers = filterSub === 'all'
    ? servers
    : filterSub === '__manual__'
      ? servers.filter(s => !s.subId)
      : servers.filter(s => s.subId === filterSub)

  // ── Delete node ──────────────────────────────────────────────────────────
  const handleDelete = (id) => {
    if (activeServer?.id === id) onDisconnect()
    setServers(prev => prev.filter(s => s.id !== id))
  }

  const handleConnect = (server) => {
    if (activeServer?.id === server.id) onDisconnect()
    else onConnect(server)
  }

  const handleTestLatency = async (server) => {
    setServers(prev => prev.map(s => s.id === server.id ? { ...s, latency: 'testing...' } : s))
    try {
      const result = await invoke('test_latency', {
        host: server.address,
        port: server.port,
        httpProxyPort: null,
      })
      const ms = result.tcp_ms !== undefined ? result.tcp_ms : null
      setServers(prev => prev.map(s => s.id === server.id
        ? { ...s, latency: ms !== null ? `${ms}ms` : '超时' }
        : s
      ))
    } catch (_) {
      setServers(prev => prev.map(s => s.id === server.id ? { ...s, latency: '失败' } : s))
    }
  }


  // ── Add subscription ──────────────────────────────────────────────────────
  const handleAddSubscription = async () => {
    if (!subUrl.trim()) return
    setImporting(true)
    setImportError('')
    try {
      const parsed = await invoke('fetch_subscription', { url: subUrl.trim() })
      if (!parsed || parsed.length === 0) {
        setImportError('订阅解析结果为空，请检查链接是否正确')
        return
      }
      const subId = genId()
      let name = subName.trim()
      if (!name) {
        try { name = new URL(subUrl.trim()).hostname } catch { name = '未命名订阅' }
      }
      const sub = { id: subId, name, url: subUrl.trim(), nodeCount: parsed.length, updatedAt: Date.now() }
      setSubscriptions(prev => [...prev, sub])
      setServers(prev => [
        ...prev,
        ...parsed.map(s => ({ ...s, id: genId(), latency: null, source: 'subscription', subId, subName: name })),
      ])
      setShowSubModal(false)
      setSubUrl('')
      setSubName('')
    } catch (err) {
      const errStr = `${err}`
      setImportError(errStr)
      // Detect if this is a subconverter-related issue
      if (errStr.includes('subconverter') || errStr.includes('Clash')) {
        setImportError(errStr + '\n__NEED_SUBCONVERTER__')
      }
    } finally {
      setImporting(false)
    }
  }

  // ── Update (refresh) subscription ────────────────────────────────────────
  const handleUpdateSubscription = async (sub) => {
    setUpdatingId(sub.id)
    try {
      const parsed = await invoke('fetch_subscription', { url: sub.url })
      if (!parsed || parsed.length === 0) {
        setUpdateError('更新失败：订阅返回空节点列表')
        setTimeout(() => setUpdateError(''), 4000)
        return
      }
      const newNodes = parsed.map(s => ({
        ...s, id: genId(), latency: null, source: 'subscription', subId: sub.id, subName: sub.name,
      }))
      setServers(prev => [...prev.filter(s => s.subId !== sub.id), ...newNodes])
      setSubscriptions(prev => prev.map(s =>
        s.id === sub.id ? { ...s, nodeCount: parsed.length, updatedAt: Date.now() } : s
      ))
    } catch (err) {
      setUpdateError(`更新失败：${err}`)
      setTimeout(() => setUpdateError(''), 4000)
    } finally {
      setUpdatingId(null)
    }
  }

  // ── Delete subscription ───────────────────────────────────────────────────
  const handleDeleteSubscription = (sub) => {
    setConfirmDelete(sub)
  }

  const confirmDeleteSubscription = () => {
    const sub = confirmDelete
    if (!sub) return
    if (activeServer?.subId === sub.id) onDisconnect()
    setServers(prev => prev.filter(s => s.subId !== sub.id))
    setSubscriptions(prev => prev.filter(s => s.id !== sub.id))
    if (filterSub === sub.id) setFilterSub('all')
    setConfirmDelete(null)
  }

  // ── Parse share link ──────────────────────────────────────────────────────
  const handleImportLink = async () => {
    const links = proxyLink.trim().split(/[\n\r]+/).filter(Boolean)
    if (!links.length) return
    setImporting(true)
    setImportError('')
    const results = []
    const errors  = []
    for (const link of links) {
      try {
        const parsed = await invoke('parse_proxy_link', { link: link.trim() })
        results.push({ ...parsed, id: genId(), latency: null, source: 'link' })
      } catch (err) {
        errors.push(`${link.slice(0, 30)}… → ${err}`)
      }
    }
    if (results.length) {
      setServers(prev => [...prev, ...results])
      setShowLinkModal(false)
      setProxyLink('')
    }
    if (errors.length) setImportError(errors.join('\n'))
    setImporting(false)
  }

  const handleAddManual = (serverData) => {
    setServers(prev => [...prev, { ...serverData, id: genId(), latency: null, source: 'manual' }])
    setShowAddModal(false)
  }

  // ── Render ────────────────────────────────────────────────────────────────
  return (
    <div className="chat-container">
      <div className="page-header">
        <div>
          <div className="page-title">节点管理</div>
          <div className="page-subtitle">
            共 {servers.length} 个节点{subscriptions.length > 0 && `，${subscriptions.length} 个订阅`}
          </div>
        </div>
        <div style={{ display: 'flex', gap: 'var(--space-sm)' }}>
          <button className="btn btn-secondary btn-sm" onClick={() => { setShowLinkModal(true); setImportError('') }} id="add-link-btn">
            🔗 粘贴链接
          </button>
          <button className="btn btn-secondary btn-sm" onClick={() => { setShowSubModal(true); setImportError('') }} id="add-sub-btn">
            📡 添加订阅
          </button>
          <button className="btn btn-primary btn-sm" onClick={() => setShowAddModal(true)} id="add-server-btn">
            ➕ 手动添加
          </button>
        </div>
      </div>

      {/* Tab bar */}
      <div style={{ display: 'flex', gap: 0, padding: '0 var(--space-lg)', borderBottom: '1px solid var(--border)', marginBottom: 'var(--space-sm)' }}>
        {[['nodes', `节点 (${servers.length})`], ['subs', `订阅 (${subscriptions.length})`]].map(([key, label]) => (
          <button key={key} onClick={() => setTab(key)} style={{
            background: 'none', border: 'none', cursor: 'pointer', padding: '8px 16px',
            fontWeight: tab === key ? 600 : 400, fontSize: '0.9rem',
            color: tab === key ? 'var(--primary)' : 'var(--text-secondary)',
            borderBottom: tab === key ? '2px solid var(--primary)' : '2px solid transparent',
            marginBottom: -1,
          }}>{label}</button>
        ))}
      </div>

      {/* ── Nodes Tab ── */}
      {tab === 'nodes' && (
        <div className="servers-container">
          {subscriptions.length > 0 && (
            <div style={{ display: 'flex', gap: 'var(--space-xs)', flexWrap: 'wrap', marginBottom: 'var(--space-sm)' }}>
              <FilterPill label="全部" active={filterSub === 'all'} onClick={() => setFilterSub('all')} count={servers.length} />
              {servers.filter(s => !s.subId).length > 0 && (
                <FilterPill label="手动添加" active={filterSub === '__manual__'} onClick={() => setFilterSub('__manual__')}
                  count={servers.filter(s => !s.subId).length} />
              )}
              {subscriptions.map(sub => (
                <FilterPill key={sub.id} label={sub.name} active={filterSub === sub.id}
                  onClick={() => setFilterSub(sub.id)} count={servers.filter(s => s.subId === sub.id).length} />
              ))}
            </div>
          )}

          {displayedServers.length === 0 ? (
            <div className="empty-state" style={{ minHeight: '340px' }}>
              <div className="empty-state-icon">🌐</div>
              <h3>还没有节点</h3>
              <p style={{ marginBottom: 'var(--space-lg)' }}>粘贴分享链接、导入订阅，或手动填写节点信息</p>
              <div style={{ display: 'flex', gap: 'var(--space-sm)', flexWrap: 'wrap', justifyContent: 'center' }}>
                <button className="btn btn-secondary" onClick={() => setShowLinkModal(true)}>🔗 粘贴链接</button>
                <button className="btn btn-secondary" onClick={() => setShowSubModal(true)}>📡 添加订阅</button>
                <button className="btn btn-primary" onClick={() => setShowAddModal(true)}>➕ 手动添加</button>
              </div>
            </div>
          ) : (
            <div className="servers-grid">
              {displayedServers.map(server => (
                <ServerCard
                  key={server.id}
                  server={server}
                  active={activeServer?.id === server.id}
                  onConnect={() => handleConnect(server)}
                  onDisconnect={() => onDisconnect()}
                  onTest={() => handleTestLatency(server)}
                  onDelete={() => handleDelete(server.id)}
                />
              ))}
            </div>
          )}
        </div>
      )}

      {/* ── Subscriptions Tab ── */}
      {tab === 'subs' && (
        <div className="servers-container">
          {subscriptions.length === 0 ? (
            <div className="empty-state" style={{ minHeight: '340px' }}>
              <div className="empty-state-icon">📡</div>
              <h3>还没有订阅</h3>
              <p style={{ marginBottom: 'var(--space-lg)' }}>添加订阅 URL，自动拉取并解析所有节点</p>
              <button className="btn btn-primary" onClick={() => setShowSubModal(true)}>📡 添加订阅</button>
            </div>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-sm)' }}>
              {subscriptions.map(sub => (
                <SubCard
                  key={sub.id}
                  sub={sub}
                  nodeCount={servers.filter(s => s.subId === sub.id).length}
                  updating={updatingId === sub.id}
                  onUpdate={() => handleUpdateSubscription(sub)}
                  onDelete={() => handleDeleteSubscription(sub)}
                  onFilter={() => { setTab('nodes'); setFilterSub(sub.id) }}
                />
              ))}
              {updateError && (
                <div style={{
                  padding: '8px 14px', borderRadius: 'var(--radius-sm)',
                  fontSize: 'var(--font-size-sm)', color: '#f87171',
                  background: 'rgba(239,68,68,0.1)', border: '1px solid rgba(239,68,68,0.25)',
                }}>
                  {updateError}
                </div>
              )}
              <button className="btn btn-secondary" style={{ alignSelf: 'flex-start', marginTop: 'var(--space-sm)' }}
                onClick={() => { setShowSubModal(true); setImportError('') }}>
                ＋ 再添加订阅
              </button>
            </div>
          )}
        </div>
      )}

      {/* Paste Link Modal */}
      {showLinkModal && (
        <div className="modal-overlay" onClick={() => setShowLinkModal(false)}>
          <div className="modal" onClick={e => e.stopPropagation()} style={{ maxWidth: 560 }}>
            <div className="modal-title">🔗 粘贴分享链接</div>
            <p style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-tertiary)', margin: '0 0 var(--space-md)' }}>
              支持 <code>vmess://</code> <code>vless://</code> <code>trojan://</code> <code>ss://</code>，每行一个
            </p>
            <div className="settings-field">
              <textarea
                className="settings-input"
                rows={6}
                placeholder={'vmess://eyJ2IjoiMiIsInBzIjoi...\nvless://uuid@host:port?...\ntrojan://pass@host:443#name'}
                value={proxyLink}
                onChange={e => setProxyLink(e.target.value)}
                style={{ resize: 'vertical', fontFamily: 'monospace', fontSize: '0.8rem' }}
                id="proxy-link-input"
              />
            </div>
            {importError && (
              <div style={{ color: 'var(--error)', fontSize: 'var(--font-size-sm)', marginTop: 'var(--space-sm)', whiteSpace: 'pre-wrap' }}>
                ⚠️ {importError}
              </div>
            )}
            <div className="modal-actions">
              <button className="btn btn-secondary" onClick={() => setShowLinkModal(false)}>取消</button>
              <button className="btn btn-primary" onClick={handleImportLink} disabled={importing} id="confirm-link-btn">
                {importing ? '⏳ 解析中...' : '📥 导入节点'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Add Subscription Modal */}
      {showSubModal && (
        <div className="modal-overlay" onClick={() => setShowSubModal(false)}>
          <div className="modal" onClick={e => e.stopPropagation()}>
            <div className="modal-title">📡 添加订阅</div>
            <div className="settings-field">
              <label className="settings-label">订阅名称（可选）</label>
              <input
                className="settings-input"
                type="text"
                placeholder="我的机场"
                value={subName}
                onChange={e => setSubName(e.target.value)}
                id="sub-name-input"
              />
            </div>
            <div className="settings-field">
              <label className="settings-label">订阅地址 URL</label>
              <input
                className="settings-input"
                type="url"
                placeholder="https://example.com/subscribe?token=xxx"
                value={subUrl}
                onChange={e => setSubUrl(e.target.value)}
                id="sub-url-input"
              />
            </div>
            <p style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-tertiary)', marginTop: 'var(--space-sm)' }}>
              支持 Base64 编码订阅及纯文本链接列表，自动解析 VMess / VLESS / Trojan / SS 节点
            </p>
            {importError && (
              <div style={{ color: 'var(--error)', fontSize: 'var(--font-size-sm)', marginTop: 'var(--space-sm)', whiteSpace: 'pre-wrap' }}>
                ⚠️ {importError.replace('__NEED_SUBCONVERTER__', '').trim()}
                {importError.includes('__NEED_SUBCONVERTER__') && onNavigate && (
                  <div style={{ marginTop: 'var(--space-sm)' }}>
                    <button
                      className="btn btn-primary btn-sm"
                      onClick={() => { setShowSubModal(false); onNavigate('tools') }}
                      style={{ fontSize: '0.8rem' }}
                    >
                      ⚙️ 前往设置安装转换工具
                    </button>
                  </div>
                )}
              </div>
            )}
            <div className="modal-actions">
              <button className="btn btn-secondary" onClick={() => setShowSubModal(false)}>取消</button>
              <button className="btn btn-primary" onClick={handleAddSubscription} disabled={importing} id="confirm-sub-btn">
                {importing ? '⏳ 导入中...' : '📥 导入节点'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Manual Add Modal */}
      {showAddModal && (
        <AddServerModal
          onClose={() => setShowAddModal(false)}
          onAdd={handleAddManual}
        />
      )}

      {/* Delete Confirm Modal */}
      {confirmDelete && (
        <div className="modal-overlay" onClick={() => setConfirmDelete(null)}>
          <div className="modal" onClick={e => e.stopPropagation()} style={{ maxWidth: 420 }}>
            <div className="modal-title">⚠️ 确认删除</div>
            <p style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-secondary)', lineHeight: 1.7, margin: '0 0 var(--space-md)' }}>
              确定要删除订阅「<strong>{confirmDelete.name}</strong>」吗？
              <br />
              其下 <strong>{confirmDelete.nodeCount}</strong> 个节点也将一并删除，此操作不可撤销。
            </p>
            <div className="modal-actions">
              <button className="btn btn-secondary" onClick={() => setConfirmDelete(null)}>取消</button>
              <button className="btn btn-danger" onClick={confirmDeleteSubscription} id="confirm-delete-sub-btn"
                style={{ background: 'rgba(239,68,68,0.8)', borderColor: 'rgba(239,68,68,0.6)' }}>
                🗑 确认删除
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

// ── Filter Pill ───────────────────────────────────────────────────────────────

function FilterPill({ label, active, onClick, count }) {
  return (
    <button onClick={onClick} style={{
      border: `1px solid ${active ? 'var(--primary)' : 'var(--border)'}`,
      background: active ? 'var(--primary)' : 'transparent',
      color: active ? '#fff' : 'var(--text-secondary)',
      borderRadius: 20, padding: '3px 12px', fontSize: '0.8rem',
      cursor: 'pointer', transition: 'all 0.15s',
    }}>
      {label}{count !== undefined ? ` (${count})` : ''}
    </button>
  )
}

// ── Subscription Card ─────────────────────────────────────────────────────────

function SubCard({ sub, nodeCount, updating, onUpdate, onDelete, onFilter }) {
  return (
    <div style={{
      background: 'var(--surface)', border: '1px solid var(--border)',
      borderRadius: 'var(--radius-md)', padding: 'var(--space-md)',
      display: 'flex', alignItems: 'center', gap: 'var(--space-md)',
    }}>
      <div style={{ fontSize: '1.5rem' }}>📡</div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontWeight: 600, marginBottom: 2 }}>{sub.name}</div>
        <div style={{ fontSize: '0.75rem', color: 'var(--text-tertiary)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {sub.url}
        </div>
        <div style={{ fontSize: '0.75rem', color: 'var(--text-secondary)', marginTop: 2 }}>
          {nodeCount} 个节点 · 更新于 {fmtTime(sub.updatedAt)}
        </div>
      </div>
      <div style={{ display: 'flex', gap: 'var(--space-xs)', flexShrink: 0 }}>
        <button className="btn btn-secondary btn-sm" onClick={onFilter} title="查看该订阅节点">🔍</button>
        <button className="btn btn-secondary btn-sm" onClick={onUpdate} disabled={updating} title="更新订阅">
          {updating ? '⏳' : '🔄'}
        </button>
        <button className="btn btn-secondary btn-sm" onClick={onDelete} title="删除订阅">🗑</button>
      </div>
    </div>
  )
}

// ── Server Card ───────────────────────────────────────────────────────────────

function ServerCard({ server, active, onConnect, onDisconnect, onTest, onDelete }) {
  const [expanded, setExpanded] = useState(false)

  return (
    <div className={`server-card ${active ? 'active' : ''}`}>
      <div className="server-card-header">
        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-sm)', minWidth: 0 }}>
          <span className={`status-dot ${active ? 'online' : 'offline'}`} />
          <span className="server-name" title={server.name}>{server.name}</span>
        </div>
        <span className="server-protocol">{server.protocol?.toUpperCase()}</span>
      </div>

      <div className="server-info">
        <div className="server-info-row">
          <span>📍</span>
          <span style={{ fontFamily: 'monospace', fontSize: '0.8rem' }}>{server.address}:{server.port}</span>
        </div>
        {server.network && (
          <div className="server-info-row">
            <span>🔌</span>
            <span>{server.network?.toUpperCase()}{server.security ? ` + ${server.security?.toUpperCase()}` : ''}</span>
          </div>
        )}
        {server.latency && (
          <div className="server-info-row">
            <span>⚡</span>
            <span className={`server-latency ${getLatencyClass(server.latency)}`}>{server.latency}</span>
          </div>
        )}
        {server.subName && (
          <div className="server-info-row">
            <span>📡</span>
            <span style={{ color: 'var(--text-tertiary)', fontSize: '0.75rem' }}>{server.subName}</span>
          </div>
        )}
      </div>

      {expanded && (
        <div style={{
          background: 'rgba(0,0,0,0.2)', borderRadius: 'var(--radius-sm)',
          padding: 'var(--space-sm)', fontSize: '0.75rem', fontFamily: 'monospace',
          color: 'var(--text-tertiary)', marginBottom: 'var(--space-sm)', lineHeight: 1.6,
        }}>
          {server.uuid             && <div>UUID: {server.uuid}</div>}
          {server.password         && <div>密码: {'•'.repeat(8)}</div>}
          {server.sni              && <div>SNI: {server.sni}</div>}
          {server.flow             && <div>Flow: {server.flow}</div>}
          {server.path             && <div>Path: {server.path}</div>}
          {server.fingerprint      && <div>Fingerprint: {server.fingerprint}</div>}
          {server.realityPublicKey && <div>PublicKey: {server.realityPublicKey?.slice(0, 20)}...</div>}
        </div>
      )}

      <div className="server-card-actions">
        <button
          className={`btn btn-sm ${active ? 'btn-danger' : 'btn-primary'}`}
          onClick={active ? onDisconnect : onConnect}
          style={{ flex: 1 }}
        >
          {active ? '⏹ 断开' : '▶ 连接'}
        </button>
        <button className="btn btn-secondary btn-sm" onClick={onTest} title="测速">⚡</button>
        <button className="btn btn-secondary btn-sm" onClick={() => setExpanded(v => !v)} title={expanded ? '收起' : '详情'}>
          {expanded ? '▲' : '▼'}
        </button>
        <button className="btn btn-secondary btn-sm" onClick={onDelete} title="删除">🗑</button>
      </div>
    </div>
  )
}

// ── Manual Add Modal ──────────────────────────────────────────────────────────

function AddServerModal({ onClose, onAdd }) {
  const [protocol, setProtocol] = useState('vless')
  const [base, setBase]   = useState({ name: '', address: '', port: '443' })
  const [extra, setExtra] = useState(PROTOCOL_DEFAULTS.vless)

  const handleProtocolChange = (p) => {
    setProtocol(p)
    setExtra(PROTOCOL_DEFAULTS[p] || {})
    setBase(prev => ({ ...prev, port: p === 'shadowsocks' ? '8388' : '443' }))
  }

  const handleSubmit = (e) => {
    e?.preventDefault()
    if (!base.address) return
    onAdd({ name: base.name || `${protocol}-${base.address}:${base.port}`, protocol, address: base.address, port: parseInt(base.port) || 443, ...extra })
  }

  const field = (label, key, placeholder, type = 'text', obj, setObj) => (
    <div className="settings-field" key={key}>
      <label className="settings-label">{label}</label>
      <input className="settings-input" type={type} placeholder={placeholder}
        value={obj[key] ?? ''} onChange={e => setObj(prev => ({ ...prev, [key]: e.target.value }))} />
    </div>
  )

  const sel = (label, key, options, obj, setObj) => (
    <div className="settings-field" key={key}>
      <label className="settings-label">{label}</label>
      <select className="settings-select" value={obj[key] ?? ''}
        onChange={e => setObj(prev => ({ ...prev, [key]: e.target.value }))}>
        {options.map(o => Array.isArray(o)
          ? <option key={o[0]} value={o[0]}>{o[1]}</option>
          : <option key={o} value={o}>{o}</option>
        )}
      </select>
    </div>
  )

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()} style={{ maxWidth: 520, maxHeight: '80vh', overflowY: 'auto' }}>
        <div className="modal-title">➕ 手动添加节点</div>
        <form onSubmit={handleSubmit}>
          {field('节点名称（可选）', 'name', '香港节点 01', 'text', base, setBase)}
          <div className="settings-field">
            <label className="settings-label">协议</label>
            <select className="settings-select" value={protocol} onChange={e => handleProtocolChange(e.target.value)}>
              <option value="vless">VLESS</option>
              <option value="vmess">VMess</option>
              <option value="trojan">Trojan</option>
              <option value="shadowsocks">Shadowsocks</option>
            </select>
          </div>
          {field('服务器地址', 'address', 'example.com 或 1.2.3.4', 'text', base, setBase)}
          {field('端口', 'port', '443', 'number', base, setBase)}

          {protocol === 'vless' && <>
            {field('UUID', 'uuid', 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx', 'text', extra, setExtra)}
            {sel('加密', 'encryption', ['none'], extra, setExtra)}
            {sel('Flow', 'flow', [['', '（无）'], 'xtls-rprx-vision', 'xtls-rprx-vision-udp443'], extra, setExtra)}
            {sel('传输', 'network', ['tcp', 'ws', 'grpc', 'http'], extra, setExtra)}
            {sel('安全', 'security', ['none', 'tls', 'reality'], extra, setExtra)}
            {(extra.security === 'tls' || extra.security === 'reality') && <>
              {field('SNI / 服务器名称', 'sni', 'example.com', 'text', extra, setExtra)}
              {sel('Fingerprint', 'fingerprint', ['chrome', 'firefox', 'safari', 'ios', 'edge', 'random'], extra, setExtra)}
            </>}
            {extra.network === 'ws' && <>
              {field('WS Path', 'path', '/', 'text', extra, setExtra)}
              {field('WS Host', 'host', 'example.com', 'text', extra, setExtra)}
            </>}
            {extra.network === 'grpc' && field('gRPC ServiceName', 'path', 'grpc', 'text', extra, setExtra)}
            {extra.security === 'reality' && <>
              {field('REALITY PublicKey', 'realityPublicKey', 'Base64 公钥', 'text', extra, setExtra)}
              {field('REALITY ShortId', 'realityShortId', '短 ID（可留空）', 'text', extra, setExtra)}
            </>}
          </>}

          {protocol === 'vmess' && <>
            {field('UUID', 'uuid', 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx', 'text', extra, setExtra)}
            {field('alterId（通常填 0）', 'alterId', '0', 'number', extra, setExtra)}
            {sel('加密', 'encryption', ['auto', 'aes-128-gcm', 'chacha20-poly1305', 'none'], extra, setExtra)}
            {sel('传输', 'network', ['tcp', 'ws', 'grpc', 'http'], extra, setExtra)}
            {sel('安全', 'security', ['none', 'tls'], extra, setExtra)}
            {extra.security === 'tls' && field('SNI', 'sni', 'example.com', 'text', extra, setExtra)}
            {extra.network === 'ws' && <>
              {field('WS Path', 'path', '/', 'text', extra, setExtra)}
              {field('WS Host', 'host', 'example.com', 'text', extra, setExtra)}
            </>}
          </>}

          {protocol === 'trojan' && <>
            {field('密码', 'password', 'your-trojan-password', 'text', extra, setExtra)}
            {sel('传输', 'network', ['tcp', 'ws', 'grpc'], extra, setExtra)}
            {field('SNI', 'sni', 'example.com', 'text', extra, setExtra)}
            {sel('Fingerprint', 'fingerprint', ['chrome', 'firefox', 'safari', 'ios', 'edge', 'random'], extra, setExtra)}
            {extra.network === 'ws' && field('WS Path', 'path', '/', 'text', extra, setExtra)}
          </>}

          {protocol === 'shadowsocks' && <>
            {field('密码', 'password', 'your-password', 'text', extra, setExtra)}
            {sel('加密方式', 'encryption', [
              'aes-256-gcm', 'aes-128-gcm', 'chacha20-poly1305',
              'chacha20-ietf-poly1305', '2022-blake3-aes-256-gcm', '2022-blake3-chacha20-poly1305',
            ], extra, setExtra)}
          </>}

          <div className="modal-actions">
            <button type="button" className="btn btn-secondary" onClick={onClose}>取消</button>
            <button type="submit" className="btn btn-primary" id="confirm-add-btn">✅ 添加节点</button>
          </div>
        </form>
      </div>
    </div>
  )
}
