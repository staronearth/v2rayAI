export default function StatusBar({ connectionStatus, activeServer, settings }) {
  return (
    <div className="status-bar">
      <div className="status-bar-left">
        <div className="status-item">
          <span className={`status-dot ${connectionStatus === 'connected' ? 'online' : 'offline'}`} />
          <span>{connectionStatus === 'connected' ? '代理运行中' : '代理未启动'}</span>
        </div>
        {activeServer && (
          <div className="status-item">
            <span>📡</span>
            <span>{activeServer.protocol?.toUpperCase()} · {activeServer.address}:{activeServer.port}</span>
          </div>
        )}
      </div>
      <div className="status-bar-right">
        <div className="status-item">
          <span>🔌</span>
          <span>HTTP: {settings.httpPort} | SOCKS: {settings.socksPort}</span>
        </div>
        <div className="status-item">
          <span>🛡️</span>
          <span>
            {settings.routingMode === 'global' ? '全局代理' :
             settings.routingMode === 'rule' ? '规则模式' : '直连模式'}
          </span>
        </div>
        <div className="status-item">
          <span>⚡</span>
          <span>{settings.coreType?.toUpperCase() || 'Xray'}</span>
        </div>
      </div>
    </div>
  )
}
