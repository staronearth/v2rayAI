import { useState, useEffect, useRef } from 'react'
import { listen } from '@tauri-apps/api/event'

export default function TrafficPage() {
  const [events, setEvents] = useState([])
  const [isPaused, setIsPaused] = useState(false)
  const [searchTerm, setSearchTerm] = useState('')
  const [autoScroll, setAutoScroll] = useState(true)

  const eventsRef = useRef([])
  const isPausedRef = useRef(false)
  const tbodyRef = useRef(null)

  useEffect(() => {
    isPausedRef.current = isPaused
  }, [isPaused])

  useEffect(() => {
    let unlisten = null

    const setup = async () => {
      unlisten = await listen('traffic-event', (e) => {
        if (isPausedRef.current) return

        const newEvent = e.payload
        eventsRef.current = [newEvent, ...eventsRef.current].slice(0, 500)
        
        // Batch React updates if necessary, but this simple approach is fine for general use
        setEvents([...eventsRef.current])
      })
    }

    setup()

    return () => {
      if (unlisten) unlisten()
    }
  }, [])

  const handleClear = () => {
    eventsRef.current = []
    setEvents([])
  }

  const filteredEvents = events.filter(e => {
    if (!searchTerm) return true
    const term = searchTerm.toLowerCase()
    return e.host.toLowerCase().includes(term) || e.route.toLowerCase().includes(term)
  })

  return (
    <div className="chat-container">
      <div className="page-header" style={{ display: 'flex', justifyContent: 'space-between' }}>
        <div>
          <div className="page-title">流量监控</div>
          <div className="page-subtitle">实时拦截与代理分析（显示最近 500 条）</div>
        </div>
        <div style={{ display: 'flex', gap: 'var(--space-sm)' }}>
          <input
            className="settings-input"
            style={{ width: '200px' }}
            placeholder="🔍 搜索域名或来源..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
          />
          <button 
            className={`btn ${isPaused ? 'btn-primary' : 'btn-secondary'}`} 
            onClick={() => setIsPaused(!isPaused)}
          >
            {isPaused ? '▶️ 继续' : '⏸ 暂停'}
          </button>
          <button className="btn btn-secondary" onClick={handleClear}>
            🗑 清空
          </button>
        </div>
      </div>

      <div className="settings-container page-enter" style={{ overflow: 'hidden', padding: 0, display: 'flex', flexDirection: 'column' }}>
        <div style={{ flex: 1, overflowY: 'auto' }} ref={tbodyRef}>
          <table style={{ width: '100%', borderCollapse: 'collapse', textAlign: 'left', fontSize: '0.85rem' }}>
            <thead style={{ position: 'sticky', top: 0, background: 'var(--bg-card)', zIndex: 10 }}>
              <tr>
                <th style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)' }}>时间</th>
                <th style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)' }}>协议</th>
                <th style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)' }}>目标地址</th>
                <th style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)' }}>端口</th>
                <th style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)', textAlign: 'right' }}>路由结果</th>
              </tr>
            </thead>
            <tbody>
              {filteredEvents.length === 0 ? (
                <tr>
                  <td colSpan={5} style={{ textAlign: 'center', padding: 'var(--space-xl)', color: 'var(--text-muted)' }}>
                    没有捕获到流量，请确保已连接节点并正在浏览网页
                  </td>
                </tr>
              ) : (
                filteredEvents.map((evt, idx) => {
                  const d = new Date(evt.timestamp)
                  const timeStr = `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`
                  
                  let badgeClass = 'badge-secondary'
                  let routeLabel = evt.route
                  
                  if (evt.route.includes('proxy')) {
                    badgeClass = 'badge-accent'
                    routeLabel = '✈️ 代理'
                  } else if (evt.route.includes('direct')) {
                    badgeClass = 'badge-success'
                    routeLabel = '🌐 直连'
                  } else if (evt.route.includes('block') || evt.route.includes('reject')) {
                    badgeClass = 'badge-error'
                    routeLabel = '🛡️ 拦截'
                  }

                  return (
                    <tr key={idx} style={{ borderBottom: '1px solid rgba(255,255,255,0.03)' }}>
                      <td style={{ padding: '6px 12px', color: 'var(--text-tertiary)', whiteSpace: 'nowrap' }}>{timeStr}</td>
                      <td style={{ padding: '6px 12px', color: 'var(--text-secondary)', textTransform: 'uppercase' }}>{evt.network}</td>
                      <td style={{ padding: '6px 12px', fontWeight: 500, fontFamily: 'monospace' }}>{evt.host}</td>
                      <td style={{ padding: '6px 12px', color: 'var(--text-tertiary)' }}>{evt.port}</td>
                      <td style={{ padding: '6px 12px', textAlign: 'right' }}>
                        <span style={{ 
                          fontSize: '0.75rem', padding: '2px 8px', borderRadius: '4px',
                          display: 'inline-block', minWidth: '60px', textAlign: 'center',
                          background: badgeClass === 'badge-accent' ? 'rgba(99,102,241,0.15)' : 
                                      badgeClass === 'badge-success' ? 'rgba(16,185,129,0.15)' :
                                      badgeClass === 'badge-error' ? 'rgba(239,68,68,0.15)' : 'rgba(255,255,255,0.1)',
                          color: badgeClass === 'badge-accent' ? 'var(--accent)' :
                                 badgeClass === 'badge-success' ? '#10b981' :
                                 badgeClass === 'badge-error' ? '#ef4444' : 'var(--text-secondary)'
                        }}>
                          {routeLabel}
                        </span>
                      </td>
                    </tr>
                  )
                })
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  )
}
