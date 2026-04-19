import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

// Provider presets that use OpenAI-compatible chat/completions APIs.
const PROVIDER_PRESETS = {
  openai: {
    baseUrl: 'https://api.openai.com/v1',
    model: 'gpt-5.4',
    keyPlaceholder: 'sk-...',
    models: ['gpt-5.4', 'gpt-5.4-mini', 'gpt-5.4-nano', 'gpt-5.4-pro'],
  },
  deepseek: {
    baseUrl: 'https://api.deepseek.com',
    model: 'deepseek-chat',
    keyPlaceholder: 'sk-...',
    models: ['deepseek-chat', 'deepseek-reasoner'],
  },
  ollama: {
    baseUrl: 'http://localhost:11434/v1',
    model: 'qwen3:7b',
    keyPlaceholder: '(本地无需 Key，留空即可)',
    models: ['qwen3:7b', 'qwen3:32b', 'llama4:8b', 'gemma4:12b', 'gemma3:4b', 'deepseek-r1:7b', 'phi4-mini'],
  },
  custom: {
    baseUrl: '',
    model: '',
    keyPlaceholder: 'your-api-key',
    models: [],
  },
}

export default function SettingsPage({ settings, setSettings }) {
  const [savedToast, setSavedToast] = useState(false)
  const [testResult, setTestResult] = useState(null) // { ok: bool, message: string }
  const [coreVersion, setCoreVersion] = useState('')
  const [latestRelease, setLatestRelease] = useState(null)
  const [coreLoading, setCoreLoading] = useState('')
  const [coreError, setCoreError] = useState('')
  const [resolveResult, setResolveResult] = useState(null)

  const updateSetting = (key, value) => {
    setSettings(prev => ({ ...prev, [key]: value }))
  }

  // Switch provider → auto-fill baseUrl and model
  const handleProviderChange = (provider) => {
    const preset = PROVIDER_PRESETS[provider]
    if (preset) {
      setSettings(prev => ({
        ...prev,
        aiProvider: provider,
        aiBaseUrl: preset.baseUrl,
        aiModel: preset.model,
      }))
    } else {
      updateSetting('aiProvider', provider)
    }
  }

  const handleSave = () => {
    setSavedToast(true)
    setTimeout(() => setSavedToast(false), 2000)
  }

  const handleTestConnection = async () => {
    if (!settings.aiApiKey && settings.aiProvider !== 'ollama') {
      setTestResult({ ok: false, message: '请先填写 API Key' })
      setTimeout(() => setTestResult(null), 3000)
      return
    }

    setTestResult({ ok: null, message: '⏳ 测试中...' })
    try {
      const headers = {}
      if (settings.aiApiKey) {
        headers['Authorization'] = `Bearer ${settings.aiApiKey}`
      }
      const response = await fetch(`${settings.aiBaseUrl}/models`, { headers })
      if (response.ok) {
        setTestResult({ ok: true, message: '✅ 连接成功！API 服务可用。' })
      } else {
        setTestResult({ ok: false, message: `❌ 连接失败：HTTP ${response.status}` })
      }
    } catch (error) {
      setTestResult({ ok: false, message: `❌ 连接失败：${error.message}` })
    }
    setTimeout(() => setTestResult(null), 4000)
  }

  const handleCheckVersion = async () => {
    if (!settings.corePath) { setCoreError('请先填写内核路径'); return }
    setCoreLoading('检测中...')
    setCoreError('')
    try {
      const v = await invoke('get_core_version', { corePath: settings.corePath })
      setCoreVersion(v)
    } catch (e) { setCoreError(String(e)) }
    setCoreLoading('')
  }

  const handleCheckLatest = async () => {
    setCoreLoading('查询最新版本...')
    setCoreError('')
    try {
      const release = await invoke('fetch_latest_xray')
      setLatestRelease(release)
    } catch (e) { setCoreError(String(e)) }
    setCoreLoading('')
  }

  const handleDownload = async () => {
    if (!latestRelease) return
    const dir = settings.corePath
      ? settings.corePath.replace(/\/[^\/]+$/, '')
      : undefined
    setCoreLoading(`下载中...`)
    setCoreError('')
    try {
      let path
      if (dir) {
        path = await invoke('download_xray_core', {
          downloadUrl: latestRelease.downloadUrl,
          installDir: dir
        })
      } else {
        const result = await invoke('resolve_core')
        path = result.path
        setResolveResult(result)
      }
      updateSetting('corePath', path)
      setCoreVersion('')
      const v = await invoke('get_core_version', { corePath: path })
      setCoreVersion(v)
    } catch (e) { setCoreError(String(e)) }
    setCoreLoading('')
  }

  // Smart resolve: find existing core, download only if needed
  const handleResolveCore = async () => {
    setCoreLoading('检测中...')
    setCoreError('')
    setResolveResult(null)
    try {
      const result = await invoke('resolve_core')
      // result: { path, source, description }
      updateSetting('corePath', result.path)
      setResolveResult(result)
      setCoreVersion('')
      // Immediately verify the version
      try {
        const v = await invoke('get_core_version', { corePath: result.path })
        setCoreVersion(v)
      } catch (_) {}
    } catch (e) {
      setCoreError(String(e))
    }
    setCoreLoading('')
  }

  const currentPreset = PROVIDER_PRESETS[settings.aiProvider] || PROVIDER_PRESETS.custom

  return (
    <div className="chat-container">
      <div className="page-header">
        <div>
          <div className="page-title">设置</div>
          <div className="page-subtitle">配置 AI 服务和代理参数</div>
        </div>
        <button className="btn btn-primary btn-sm" onClick={handleSave} id="save-settings-btn">
          💾 保存设置
        </button>
      </div>

      <div className="settings-container page-enter">
        {/* AI Configuration */}
        <div className="settings-section">
          <div className="settings-section-title">
            🤖 AI 配置
          </div>
          <div className="settings-card">
            <div className="settings-field">
              <label className="settings-label">AI 服务提供商</label>
              <select
                className="settings-select"
                value={PROVIDER_PRESETS[settings.aiProvider] ? settings.aiProvider : 'custom'}
                onChange={e => handleProviderChange(e.target.value)}
                id="ai-provider-select"
              >
                <option value="openai">OpenAI</option>
                <option value="deepseek">DeepSeek</option>
                <option value="ollama">Ollama (本地)</option>
                <option value="custom">自定义</option>
              </select>
            </div>

            <div className="settings-field">
              <label className="settings-label">API Base URL</label>
              <input
                className="settings-input"
                placeholder={currentPreset.baseUrl || 'https://api.example.com/v1'}
                value={settings.aiBaseUrl}
                onChange={e => updateSetting('aiBaseUrl', e.target.value)}
                id="ai-base-url-input"
              />
            </div>

            <div className="settings-field">
              <label className="settings-label">API Key</label>
              <input
                className="settings-input"
                type="password"
                placeholder={currentPreset.keyPlaceholder}
                value={settings.aiApiKey}
                onChange={e => updateSetting('aiApiKey', e.target.value)}
                id="ai-api-key-input"
              />
            </div>

            <div className="settings-field">
              <label className="settings-label">模型</label>
              {currentPreset.models.length > 0 ? (
                <select
                  className="settings-select"
                  value={currentPreset.models.includes(settings.aiModel) ? settings.aiModel : '__custom__'}
                  onChange={e => {
                    if (e.target.value !== '__custom__') {
                      updateSetting('aiModel', e.target.value)
                    }
                  }}
                  id="ai-model-select"
                >
                  {currentPreset.models.map(m => (
                    <option key={m} value={m}>{m}</option>
                  ))}
                  {!currentPreset.models.includes(settings.aiModel) && (
                    <option value="__custom__">{settings.aiModel || '自定义模型'}</option>
                  )}
                </select>
              ) : (
                <input
                  className="settings-input"
                  placeholder="输入模型名称"
                  value={settings.aiModel}
                  onChange={e => updateSetting('aiModel', e.target.value)}
                  id="ai-model-input"
                />
              )}
              {currentPreset.models.length > 0 && !currentPreset.models.includes(settings.aiModel) && (
                <input
                  className="settings-input"
                  placeholder="或输入自定义模型名称"
                  value={settings.aiModel}
                  onChange={e => updateSetting('aiModel', e.target.value)}
                  style={{ marginTop: 'var(--space-sm)' }}
                  id="ai-model-custom-input"
                />
              )}
            </div>

            <button
              className="btn btn-secondary btn-sm"
              onClick={handleTestConnection}
              id="test-ai-btn"
            >
              🔌 测试连接
            </button>
            {testResult && (
              <div style={{
                marginTop: 'var(--space-sm)',
                padding: '6px 12px',
                borderRadius: 'var(--radius-sm)',
                fontSize: 'var(--font-size-sm)',
                background: testResult.ok === true ? 'rgba(34,197,94,0.12)' :
                            testResult.ok === false ? 'rgba(239,68,68,0.12)' :
                            'rgba(99,102,241,0.12)',
                color: testResult.ok === true ? '#4ade80' :
                       testResult.ok === false ? '#f87171' :
                       'var(--accent-light)',
                border: `1px solid ${testResult.ok === true ? 'rgba(34,197,94,0.3)' :
                                     testResult.ok === false ? 'rgba(239,68,68,0.3)' :
                                     'rgba(99,102,241,0.3)'}`,
              }}>
                {testResult.message}
              </div>
            )}
          </div>
        </div>

        {/* Core Configuration */}
        <div className="settings-section">
          <div className="settings-section-title">⚡ Xray 内核</div>
          <div className="settings-card">

            {/* Version badge */}
            {coreVersion && (
              <div style={{
                display: 'inline-flex', alignItems: 'center', gap: 8,
                background: 'rgba(99,102,241,0.15)', border: '1px solid rgba(99,102,241,0.3)',
                borderRadius: 'var(--radius-sm)', padding: '4px 12px',
                fontSize: 'var(--font-size-sm)', color: 'var(--accent)',
                marginBottom: 'var(--space-md)',
              }}>
                ✅ {coreVersion}
              </div>
            )}

            {/* Resolve source badge */}
            {resolveResult && (
              <div style={{
                display: 'inline-flex', alignItems: 'center', gap: 8,
                background: resolveResult.source === 'existing'
                  ? 'rgba(34,197,94,0.12)' : 'rgba(6,182,212,0.12)',
                border: `1px solid ${resolveResult.source === 'existing'
                  ? 'rgba(34,197,94,0.3)' : 'rgba(6,182,212,0.3)'}`,
                borderRadius: 'var(--radius-sm)', padding: '4px 12px',
                fontSize: 'var(--font-size-sm)',
                color: resolveResult.source === 'existing' ? '#4ade80' : '#22d3ee',
                marginBottom: 'var(--space-md)', marginLeft: resolveResult && coreVersion ? 8 : 0,
              }}>
                {resolveResult.source === 'existing' ? '📦' : '⬇️'} {resolveResult.description}
              </div>
            )}

            {/* Latest release info */}
            {latestRelease && (
              <div style={{
                background: 'rgba(6,182,212,0.08)', border: '1px solid rgba(6,182,212,0.2)',
                borderRadius: 'var(--radius-sm)', padding: 'var(--space-sm) var(--space-md)',
                fontSize: 'var(--font-size-sm)', marginBottom: 'var(--space-md)',
              }}>
                <div style={{ fontWeight: 600 }}>🆕 最新版本：{latestRelease.version}</div>
                <div style={{ color: 'var(--text-tertiary)', fontSize: '0.75rem', marginTop: 2 }}>
                  发布于 {new Date(latestRelease.publishedAt).toLocaleDateString('zh-CN')}
                </div>
              </div>
            )}

            {/* Error */}
            {coreError && (
              <div style={{ color: 'var(--error)', fontSize: 'var(--font-size-sm)', marginBottom: 'var(--space-md)' }}>
                ⚠️ {coreError}
              </div>
            )}

            <div className="settings-field">
              <label className="settings-label">内核类型</label>
              <select
                className="settings-select"
                value={settings.coreType}
                onChange={e => updateSetting('coreType', e.target.value)}
                id="core-type-select"
              >
                <option value="xray">Xray-core（推荐，支持 REALITY / XTLS）</option>
                <option value="v2ray">V2Ray-core（传统稳定）</option>
              </select>
            </div>

            <div className="settings-field">
              <label className="settings-label">内核路径</label>
              <div style={{ display: 'flex', gap: 'var(--space-sm)' }}>
                <input
                  className="settings-input"
                  placeholder="/usr/local/bin/xray 或 C:\xray\xray.exe"
                  value={settings.corePath}
                  onChange={e => updateSetting('corePath', e.target.value)}
                  id="core-path-input"
                  style={{ flex: 1 }}
                />
                <button
                  className="btn btn-secondary btn-sm"
                  onClick={handleCheckVersion}
                  disabled={!!coreLoading}
                  id="check-version-btn"
                >
                  {coreLoading === '检测中...' ? '⏳' : '🔍 检测版本'}
                </button>
              </div>
              <div style={{ fontSize: 'var(--font-size-xs)', color: 'var(--text-muted)', marginTop: 4 }}>
                指定 xray 可执行文件的完整路径。未安装？点击下方自动下载。
              </div>
            </div>

            {/* Smart resolve + manual download actions */}
            <div style={{ display: 'flex', gap: 'var(--space-sm)', flexWrap: 'wrap', marginTop: 'var(--space-sm)' }}>
              {/* Primary action: smart detect-or-download */}
              <button
                className="btn btn-primary btn-sm"
                onClick={handleResolveCore}
                disabled={!!coreLoading}
                id="resolve-core-btn"
                title="先检测系统已有内核，找不到再自动下载"
              >
                {coreLoading === '检测中...' ? '⏳ 检测中...' : '🔍 自动检测 / 安装'}
              </button>

              <button
                className="btn btn-secondary btn-sm"
                onClick={handleCheckLatest}
                disabled={!!coreLoading}
                id="check-latest-btn"
              >
                {coreLoading === '查询最新版本...' ? '⏳ 查询中...' : '🌐 查询最新版本'}
              </button>
              {latestRelease && (
                <button
                  className="btn btn-secondary btn-sm"
                  onClick={handleDownload}
                  disabled={!!coreLoading}
                  id="download-xray-btn"
                >
                  {coreLoading.startsWith('下载中') ? `⏳ ${coreLoading}` : `⬇️ 强制下载 ${latestRelease.version}`}
                </button>
              )}
              <a
                href="https://github.com/XTLS/Xray-core/releases"
                target="_blank"
                rel="noreferrer"
                className="btn btn-secondary btn-sm"
                style={{ textDecoration: 'none' }}
              >
                📦 GitHub 手动下载
              </a>
            </div>
          </div>
        </div>

        {/* Proxy Settings */}
        <div className="settings-section">
          <div className="settings-section-title">
            🌐 代理设置
          </div>
          <div className="settings-card">
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 'var(--space-lg)' }}>
              <div className="settings-field">
                <label className="settings-label">HTTP 代理端口</label>
                <input
                  className="settings-input"
                  type="number"
                  value={settings.httpPort}
                  onChange={e => updateSetting('httpPort', parseInt(e.target.value))}
                  id="http-port-input"
                />
              </div>

              <div className="settings-field">
                <label className="settings-label">SOCKS5 代理端口</label>
                <input
                  className="settings-input"
                  type="number"
                  value={settings.socksPort}
                  onChange={e => updateSetting('socksPort', parseInt(e.target.value))}
                  id="socks-port-input"
                />
              </div>
            </div>

            <div className="settings-field">
              <label className="settings-label">路由模式</label>
              <select
                className="settings-select"
                value={settings.routingMode}
                onChange={e => updateSetting('routingMode', e.target.value)}
                id="routing-mode-select"
              >
                <option value="rule">🛡️ 规则模式（国内直连，国外代理）</option>
                <option value="global">🌏 全局代理（所有流量走代理）</option>
                <option value="direct">🔓 直连模式（不使用代理）</option>
              </select>
            </div>
          </div>
        </div>

        {/* About */}
        <div className="settings-section">
          <div className="settings-section-title">
            ℹ️ 关于
          </div>
          <div className="settings-card">
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-md)' }}>
              <div style={{
                width: '48px',
                height: '48px',
                borderRadius: 'var(--radius-md)',
                background: 'var(--accent-gradient)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: '1.5rem',
                flexShrink: 0,
              }}>
                ⚡
              </div>
              <div>
                <div style={{ fontWeight: 600, fontSize: 'var(--font-size-md)' }}>v2rayAI</div>
                <div style={{ color: 'var(--text-tertiary)', fontSize: 'var(--font-size-sm)' }}>
                  v0.1.0 · AI 驱动的代理配置工具
                </div>
                <div style={{ color: 'var(--text-muted)', fontSize: 'var(--font-size-xs)', marginTop: '4px' }}>
                  让翻墙像喝水一样简单 🚀
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Save Toast */}
      {savedToast && (
        <div className="toast success">
          ✅ 设置已保存
        </div>
      )}
    </div>
  )
}
