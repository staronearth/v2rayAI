import { useState, useEffect, useRef, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import Sidebar from './components/Sidebar'
import StatusBar from './components/StatusBar'
import ChatPage from './pages/ChatPage'
import ServersPage from './pages/ServersPage'
import SettingsPage from './pages/SettingsPage'
import LogsPage from './pages/LogsPage'
import ToolsPage from './pages/ToolsPage'
import TrafficPage from './pages/TrafficPage'

// ── Persistent storage via tauri-plugin-store ─────────────────────────────────

let storeInstance = null

async function getStore() {
  if (storeInstance) return storeInstance
  try {
    const { load } = await import('@tauri-apps/plugin-store')
    storeInstance = await load('v2rayai-settings.json', { autoSave: true })
    return storeInstance
  } catch (e) {
    console.warn('Store not available (dev mode?):', e)
    return null
  }
}

async function loadFromStore(key, defaultValue) {
  try {
    const store = await getStore()
    if (!store) return defaultValue
    const val = await store.get(key)
    return val !== null && val !== undefined ? val : defaultValue
  } catch {
    return defaultValue
  }
}

async function saveToStore(key, value) {
  try {
    const store = await getStore()
    if (!store) return
    await store.set(key, value)
  } catch (e) {
    console.warn('Save failed:', e)
  }
}

// ── Default settings ──────────────────────────────────────────────────────────

const DEFAULT_SETTINGS = {
  aiProvider: 'openai',
  aiBaseUrl: 'https://api.openai.com/v1',
  aiApiKey: '',
  aiModel: 'gpt-5.4',
  corePath: '',
  coreType: 'xray',
  httpPort: 10808,
  socksPort: 10809,
  routingMode: 'rule',
  language: 'zh',
}

// ── App Component ─────────────────────────────────────────────────────────────

function App() {
  const [currentPage, setCurrentPage] = useState('chat')
  const [connectionStatus, setConnectionStatus] = useState('disconnected')
  const [activeServer, setActiveServer] = useState(null)
  const [servers, setServers] = useState([])
  const [subscriptions, setSubscriptions] = useState([])
  const [settings, setSettings] = useState(DEFAULT_SETTINGS)
  const [loaded, setLoaded] = useState(false)
  const [globalToast, setGlobalToast] = useState(null) // { type: 'error'|'info', message: string }

  const saveServersTimer = useRef(null)

  // ── Load persisted data on mount ────────────────────────────────────────────
  useEffect(() => {
    async function init() {
      const [savedSettings, savedServers, savedSubs] = await Promise.all([
        loadFromStore('settings', DEFAULT_SETTINGS),
        loadFromStore('servers', []),
        loadFromStore('subscriptions', []),
      ])
      
      let finalSettings = { ...DEFAULT_SETTINGS, ...savedSettings }
      
      // Zero-config: Auto-detect or install core if missing
      if (!finalSettings.corePath) {
        try {
          const result = await invoke('resolve_core')
          finalSettings.corePath = result.path
        } catch (e) {
          console.warn('Core auto-resolve failed:', e)
        }
      }

      setSettings(finalSettings)
      if (savedServers.length > 0) setServers(savedServers)
      if (savedSubs.length   > 0) setSubscriptions(savedSubs)
      setLoaded(true)
    }
    init()
  }, [])

  // ── Auto-save settings whenever they change ────────────────────────────────
  useEffect(() => {
    if (!loaded) return
    saveToStore('settings', settings)
  }, [settings, loaded])

  // ── Auto-save servers (debounced 1s) ───────────────────────────────────────
  const saveServers = useCallback((srvs) => {
    if (saveServersTimer.current) clearTimeout(saveServersTimer.current)
    saveServersTimer.current = setTimeout(() => {
      // Strip transient fields before saving
      const toSave = srvs.map(s => {
        const { latency, ...rest } = s
        return rest
      })
      saveToStore('servers', toSave)
    }, 1000)
  }, [])

  useEffect(() => {
    if (!loaded) return
    saveServers(servers)
  }, [servers, loaded, saveServers])

  // ── Auto-save subscriptions ───────────────────────────────────────────────
  useEffect(() => {
    if (!loaded) return
    saveToStore('subscriptions', subscriptions)
  }, [subscriptions, loaded])

  // ── Handlers ────────────────────────────────────────────────────────────────
  const handleConnect = async (server) => {
    try {
      if (!settings.corePath) {
        setGlobalToast({ type: 'error', message: '请先在“设置”页面配置 Xray 内核路径！' })
        setTimeout(() => setGlobalToast(null), 4000)
        return
      }
      
      setConnectionStatus('connecting')
      
      // 1. Generate and apply config
      await invoke('apply_config', {
        server: server.fullConfig || server,
        httpPort: settings.httpPort,
        socksPort: settings.socksPort,
        routingMode: settings.routingMode
      })
      
      // 2. Start Xray Core
      await invoke('start_core', { corePath: settings.corePath })
      
      // 3. Enable OS System Proxy
      await invoke('enable_proxy', {
        httpPort: settings.httpPort,
        socksPort: settings.socksPort
      })

      // 4. Start Health Monitor
      await invoke('start_health_monitor', { httpProxyPort: settings.httpPort })

      setActiveServer(server)
      setConnectionStatus('connected')
    } catch (e) {
      setGlobalToast({ type: 'error', message: `连接失败：${e}` })
      setTimeout(() => setGlobalToast(null), 5000)
      setConnectionStatus('disconnected')
    }
  }

  const handleDisconnect = async () => {
    try {
      setConnectionStatus('disconnecting')
      
      await invoke('stop_core')
      await invoke('disable_proxy')
      await invoke('stop_health_monitor')

      setActiveServer(null)
      setConnectionStatus('disconnected')
    } catch (e) {
      console.warn('Disconnect error:', e)
      setActiveServer(null)
      setConnectionStatus('disconnected')
    }
  }

  const handleAddServers = (newServers) => {
    setServers(prev => [...prev, ...newServers])
  }

  // ── Render ──────────────────────────────────────────────────────────────────
  const renderPage = () => {
    switch (currentPage) {
      case 'chat':
        return (
          <ChatPage
            settings={settings}
            onAddServers={handleAddServers}
            activeServer={activeServer}
            isConnected={connectionStatus === 'connected'}
          />
        )
      case 'servers':
        return (
          <ServersPage
            servers={servers}
            setServers={setServers}
            subscriptions={subscriptions}
            setSubscriptions={setSubscriptions}
            activeServer={activeServer}
            onConnect={handleConnect}
            onDisconnect={handleDisconnect}
            onNavigate={setCurrentPage}
          />
        )
      case 'traffic':
        return (
          <TrafficPage />
        )
      case 'logs':
        return (
          <LogsPage settings={settings} />
        )
      case 'tools':
        return (
          <ToolsPage />
        )
      case 'settings':
        return (
          <SettingsPage
            settings={settings}
            setSettings={setSettings}
          />
        )
      default:
        return <ChatPage settings={settings} onAddServers={handleAddServers} />
    }
  }

  // Show nothing until loaded to prevent flash of default values
  if (!loaded) {
    return (
      <div className="app-layout" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center' }}>
        <div style={{ color: 'var(--text-tertiary)', fontSize: '0.9rem' }}>⚡ 加载中...</div>
      </div>
    )
  }

  return (
    <div className="app-layout">
      <Sidebar
        currentPage={currentPage}
        onNavigate={setCurrentPage}
        connectionStatus={connectionStatus}
        activeServer={activeServer}
      />
      <div className="main-content">
        {renderPage()}
        <StatusBar
          connectionStatus={connectionStatus}
          activeServer={activeServer}
          settings={settings}
        />
      </div>

      {/* Global Toast Notification */}
      {globalToast && (
        <div
          onClick={() => setGlobalToast(null)}
          style={{
            position: 'fixed',
            top: 20,
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 10000,
            padding: '10px 24px',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--font-size-sm)',
            fontWeight: 500,
            cursor: 'pointer',
            backdropFilter: 'blur(12px)',
            boxShadow: '0 4px 20px rgba(0,0,0,0.4)',
            maxWidth: '80vw',
            background: globalToast.type === 'error'
              ? 'rgba(239,68,68,0.15)'
              : 'rgba(99,102,241,0.15)',
            color: globalToast.type === 'error' ? '#f87171' : '#a5b4fc',
            border: `1px solid ${globalToast.type === 'error'
              ? 'rgba(239,68,68,0.3)'
              : 'rgba(99,102,241,0.3)'}`,
          }}
        >
          {globalToast.message}
        </div>
      )}
    </div>
  )
}

export default App
