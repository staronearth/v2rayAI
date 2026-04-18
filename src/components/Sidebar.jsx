import { useState } from 'react'

export default function Sidebar({ currentPage, onNavigate, connectionStatus, activeServer }) {
  const navItems = [
    { id: 'chat', icon: '🤖', label: 'AI 助手', desc: '智能配置' },
    { id: 'servers', icon: '🌐', label: '节点管理', desc: '服务器列表' },
    { id: 'traffic', icon: '📡', label: '流量监控', desc: '路由分析' },
    { id: 'logs', icon: '📋', label: '日志', desc: '运行状态' },
    { id: 'tools', icon: '🔧', label: '工具', desc: '订阅转换' },
    { id: 'settings', icon: '⚙️', label: '设置', desc: '偏好配置' },
  ]

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-logo">⚡</div>
        <div className="sidebar-title">
          <h1>v2rayAI</h1>
          <span>AI 代理配置助手</span>
        </div>
      </div>

      <nav className="sidebar-nav">
        <div className="nav-section-title">导航</div>
        {navItems.map(item => (
          <button
            key={item.id}
            className={`nav-item ${currentPage === item.id ? 'active' : ''}`}
            onClick={() => onNavigate(item.id)}
            id={`nav-${item.id}`}
          >
            <span className="nav-item-icon">{item.icon}</span>
            <span>{item.label}</span>
          </button>
        ))}
      </nav>

      <div className="sidebar-footer">
        <div className="connection-status">
          <span className={`status-dot ${connectionStatus === 'connected' ? 'online' : 'offline'}`} />
          <span style={{ flex: 1, color: connectionStatus === 'connected' ? 'var(--status-online)' : 'var(--text-tertiary)' }}>
            {connectionStatus === 'connected' ? '已连接' : '未连接'}
          </span>
          {activeServer && (
            <span style={{ fontSize: 'var(--font-size-xs)', color: 'var(--text-tertiary)' }}>
              {activeServer.name}
            </span>
          )}
        </div>
      </div>
    </aside>
  )
}
