/** 常用 MCP 服务器模板（点击自动填充 name/command/env/description） */
export interface McpEnvDef {
  /** 环境变量名 */
  key: string
  /** 示例值（输入框占位/缺失时提示） */
  placeholder: string
  /** 变量用途说明 */
  hint: string
  /** 本机常用默认值：点击模板添加/一键填入时自动带出（仅本地服务类变量） */
  defaultValue?: string
}

export interface McpTemplate {
  key: string
  name: string
  command: string[]
  env?: Record<string, string>
  description: string
  homepage?: string
  /** 环境变量占位提示（用于 UI 展示需要配置的 env） */
  envHint?: string
  /** 环境变量结构化说明（编辑表单展示指导） */
  envDefs?: McpEnvDef[]
  /** 推荐状态：hot=热门（月下载量高/大厂官方），popular=值得推荐 */
  recommended?: 'hot' | 'popular'
  /** 流行度数据展示（如 "25.9M 次/月"） */
  popularity?: string
}

/**
 * 模板 env 默认文本（每行一个 "KEY=value"）：带出全部环境变量，
 * 有本机默认值的填默认值，其余（如密码/API Key）留空待用户补全。
 */
export function templateEnvDefaults(tpl: McpTemplate): string {
  if (!tpl.envDefs) return ''
  return tpl.envDefs.map((d) => `${d.key}=${d.defaultValue ?? ''}`).join('\n')
}

export const mcpTemplates: McpTemplate[] = [
  {
    key: 'redis',
    name: 'Redis',
    command: ['npx', '-y', 'redis-mcp'],
    description: 'Redis 数据库操作（查询/写入/键管理）',
    envHint: 'REDIS_URL=redis://localhost:6379',
    envDefs: [
      {
        key: 'REDIS_URL',
        placeholder: 'redis://localhost:6379',
        hint: 'Redis 连接地址，本机默认服务无需修改；带密码格式 redis://:密码@主机:端口',
        defaultValue: 'redis://localhost:6379',
      },
    ],
    homepage: 'https://github.com/gongrzhe/server-mcp-redis',
  },
  {
    key: 'mysql',
    name: 'MySQL',
    command: ['npx', '-y', '@benborla29/mcp-server-mysql'],
    description: 'MySQL 数据库查询与操作',
    recommended: 'popular',
    popularity: '48K 次/月',
    envHint: 'MYSQL_HOST=127.0.0.1, MYSQL_PORT=3306, MYSQL_USER=root, MYSQL_PASS=xxx, MYSQL_DB=test',
    envDefs: [
      { key: 'MYSQL_HOST', placeholder: '127.0.0.1', hint: 'MySQL 主机地址', defaultValue: '127.0.0.1' },
      { key: 'MYSQL_PORT', placeholder: '3306', hint: 'MySQL 端口', defaultValue: '3306' },
      { key: 'MYSQL_USER', placeholder: 'root', hint: '登录用户名', defaultValue: 'root' },
      { key: 'MYSQL_PASS', placeholder: '密码', hint: '登录密码（本地无密码可留空）' },
      { key: 'MYSQL_DB', placeholder: 'test', hint: '默认连接的数据库名', defaultValue: 'test' },
    ],
    homepage: 'https://github.com/benborla29/mcp-server-mysql',
  },
  {
    key: 'postgres',
    name: 'PostgreSQL',
    command: ['npx', '-y', '@henkey/postgres-mcp-server'],
    description: 'PostgreSQL 数据库查询与操作',
    recommended: 'popular',
    popularity: '9K 次/月',
    envHint: 'DATABASE_URL=postgresql://user:pass@localhost:5432/db',
    envDefs: [
      {
        key: 'DATABASE_URL',
        placeholder: 'postgresql://user:pass@localhost:5432/db',
        hint: '完整连接串，格式 postgresql://用户:密码@主机:端口/库名',
        defaultValue: 'postgresql://postgres:postgres@localhost:5432/postgres',
      },
    ],
    homepage: 'https://github.com/henkey/postgres-mcp-server',
  },
  {
    key: 'mongodb',
    name: 'MongoDB',
    command: ['npx', '-y', 'mongodb-mcp-server'],
    description: 'MongoDB 官方 MCP：查询/聚合/索引管理',
    recommended: 'hot',
    popularity: '446K 次/月',
    envHint: 'MONGODB_URI=mongodb://localhost:27017',
    envDefs: [
      {
        key: 'MONGODB_URI',
        placeholder: 'mongodb://localhost:27017',
        hint: 'MongoDB 连接串：本机默认服务无需修改；Atlas 云库用 mongodb+srv:// 开头；带认证 mongodb://用户:密码@主机:端口',
        defaultValue: 'mongodb://localhost:27017',
      },
    ],
    homepage: 'https://github.com/mongodb/mongodb-mcp-server',
  },
  {
    key: 'elasticsearch',
    name: 'Elasticsearch',
    command: ['npx', '-y', '@elastic/mcp-server-elasticsearch'],
    description: 'Elasticsearch 索引与搜索操作（Elastic 官方）',
    recommended: 'popular',
    popularity: '7.1K 次/月',
    envHint: 'ES_URL=http://localhost:9200, ES_API_KEY=xxx',
    envDefs: [
      { key: 'ES_URL', placeholder: 'http://localhost:9200', hint: 'Elasticsearch 服务地址', defaultValue: 'http://localhost:9200' },
      { key: 'ES_API_KEY', placeholder: 'API Key（可选）', hint: 'API Key（本地无认证可留空）' },
    ],
    homepage: 'https://github.com/elastic/mcp-server-elasticsearch',
  },
  {
    key: 'sqlite',
    name: 'SQLite',
    command: ['npx', '-y', 'mcp-server-sqlite', '--db', './sqlite.db'],
    description: 'SQLite 数据库（本地文件，--db 参数指定数据库文件路径）',
    recommended: 'popular',
    popularity: '2.4K 次/月',
    homepage: 'https://github.com/madnh/mcp-server-sqlite',
  },
  {
    key: 'mssql',
    name: 'MSSQL Server',
    command: ['npx', '-y', 'mssql-mcp'],
    description: 'Microsoft SQL Server 查询与操作',
    recommended: 'popular',
    popularity: '3.3K 次/月',
    envHint: 'MSSQL_HOST=localhost, MSSQL_PORT=1433, MSSQL_USER=sa, MSSQL_PASSWORD=xxx, MSSQL_DATABASE=master',
    envDefs: [
      { key: 'MSSQL_HOST', placeholder: 'localhost', hint: 'SQL Server 主机地址', defaultValue: 'localhost' },
      { key: 'MSSQL_PORT', placeholder: '1433', hint: '端口（默认 1433）', defaultValue: '1433' },
      { key: 'MSSQL_USER', placeholder: 'sa', hint: '登录用户名', defaultValue: 'sa' },
      { key: 'MSSQL_PASSWORD', placeholder: '密码', hint: '登录密码（必填）' },
      { key: 'MSSQL_DATABASE', placeholder: 'master', hint: '默认连接的数据库名', defaultValue: 'master' },
    ],
    homepage: 'https://github.com/sujaygarlanka/mssql-mcp',
  },
  {
    key: 'clickhouse',
    name: 'ClickHouse',
    command: ['npx', '-y', 'clickhouse-mcp'],
    description: 'ClickHouse 查询与分析',
    envHint: 'CLICKHOUSE_HOST=http://localhost:8123, CLICKHOUSE_USER=default, CLICKHOUSE_PASSWORD=xxx',
    envDefs: [
      { key: 'CLICKHOUSE_HOST', placeholder: 'http://localhost:8123', hint: '服务地址', defaultValue: 'http://localhost:8123' },
      { key: 'CLICKHOUSE_USER', placeholder: 'default', hint: '用户名', defaultValue: 'default' },
      { key: 'CLICKHOUSE_PASSWORD', placeholder: '密码（可选）', hint: '密码（无密码可留空）' },
    ],
    homepage: 'https://github.com/AminKhorramii/mcp-clikchouse-ts',
  },
  {
    key: 'duckdb',
    name: 'DuckDB',
    command: ['npx', '-y', '@seed-ship/duckdb-mcp-native'],
    description: 'DuckDB 本地分析数据库（列式存储，无需服务，免配置）',
    recommended: 'popular',
    popularity: '2.8K 次/月',
    homepage: 'https://github.com/theseedship/duckdb_mcp_node',
  },
  {
    key: 'kafka',
    name: 'Kafka',
    command: ['npx', '-y', 'kafka-mcp-server'],
    description: 'Apache Kafka 消息队列：topic/生产/消费',
    envHint: 'KAFKA_BOOTSTRAP_SERVERS=localhost:9092',
    envDefs: [
      {
        key: 'KAFKA_BOOTSTRAP_SERVERS',
        placeholder: 'localhost:9092',
        hint: 'Kafka broker 地址（多个用逗号分隔）',
        defaultValue: 'localhost:9092',
      },
    ],
    homepage: 'https://github.com/hasura/kafka-mcp-server',
  },
  {
    key: 'sentry',
    name: 'Sentry',
    command: ['npx', '-y', '@sentry/mcp-server'],
    description: 'Sentry 官方 MCP：错误监控、Issue 排查',
    recommended: 'hot',
    popularity: '438K 次/月',
    envHint: 'SENTRY_AUTH_TOKEN=sntrys_xxx, SENTRY_ORG=org, SENTRY_PROJECT=project',
    envDefs: [
      { key: 'SENTRY_AUTH_TOKEN', placeholder: 'sntrys_xxx', hint: 'Sentry 认证 Token（sentry.io → Settings → Auth Tokens 创建）' },
      { key: 'SENTRY_ORG', placeholder: 'org', hint: '组织名（URL 中的 slub）' },
      { key: 'SENTRY_PROJECT', placeholder: 'project', hint: '项目名' },
    ],
    homepage: 'https://github.com/getsentry/sentry-mcp',
  },
  {
    key: 'playwright',
    name: 'Playwright',
    command: ['npx', '-y', '@playwright/mcp@latest'],
    description: '浏览器自动化：打开网页、点击、截图、抓取内容',
    recommended: 'hot',
    popularity: '25.9M 次/月',
    homepage: 'https://github.com/microsoft/playwright-mcp',
  },
  {
    key: 'puppeteer',
    name: 'Puppeteer',
    command: ['npx', '-y', '@modelcontextprotocol/server-puppeteer'],
    description: '无头浏览器控制（旧版，推荐 Playwright）',
    recommended: 'hot',
    popularity: '119K 次/月',
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/puppeteer',
  },
  {
    key: 'filesystem',
    name: 'Filesystem',
    command: ['npx', '-y', '@modelcontextprotocol/server-filesystem', '~/'],
    description: '本地文件系统读写（默认工作目录 ~/，可自行修改路径）',
    recommended: 'hot',
    popularity: '1.9M 次/月',
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem',
  },
  {
    key: 'github',
    name: 'GitHub',
    command: ['npx', '-y', '@modelcontextprotocol/server-github'],
    description: 'GitHub 仓库/Issue/PR 操作',
    recommended: 'hot',
    popularity: '536K 次/月',
    envHint: 'GITHUB_PERSONAL_ACCESS_TOKEN=ghp_xxx',
    envDefs: [
      {
        key: 'GITHUB_PERSONAL_ACCESS_TOKEN',
        placeholder: 'ghp_xxx',
        hint: 'GitHub Personal Access Token（github.com → Settings → Developer settings → Tokens，勾选 repo 权限）',
      },
    ],
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/github',
  },
  {
    key: 'git',
    name: 'Git',
    command: ['npx', '-y', 'mcp-server-git'],
    description: 'Git 仓库操作：提交/分支/日志/差异',
    envHint: 'GIT_REPOSITORY=/path/to/repo',
    envDefs: [
      {
        key: 'GIT_REPOSITORY',
        placeholder: '/path/to/repo',
        hint: '要操作的 Git 仓库绝对路径',
      },
    ],
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/git',
  },
  {
    key: 'docker',
    name: 'Docker',
    command: ['npx', '-y', 'mcp-server-docker'],
    description: 'Docker 容器与镜像管理',
    recommended: 'popular',
    popularity: '2.1K 次/月',
    homepage: 'https://github.com/ckreiling/mcp-server-docker',
  },
  {
    key: 'memory',
    name: 'Memory',
    command: ['npx', '-y', '@modelcontextprotocol/server-memory'],
    description: '长期知识图谱记忆（跨会话知识存储）',
    recommended: 'hot',
    popularity: '399K 次/月',
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/memory',
  },
  {
    key: 'fetch',
    name: 'HTTP Request',
    command: ['npx', '-y', '@vrosario/http-request-mcp'],
    description: 'HTTP 请求工具：GET/POST/PUT/DELETE 调试接口',
    homepage: 'https://github.com/vitolrosario/http-request-mcp',
  },
  {
    key: 'sequential-thinking',
    name: 'Sequential Thinking',
    command: ['npx', '-y', '@modelcontextprotocol/server-sequential-thinking'],
    description: '结构化逐步推理（复杂问题分解）',
    recommended: 'hot',
    popularity: '572K 次/月',
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking',
  },
  {
    key: 'time',
    name: 'Time',
    command: ['npx', '-y', 'time-mcp'],
    description: '时间与时区查询',
    recommended: 'popular',
    popularity: '6.4K 次/月',
    homepage: 'https://github.com/yokingma/time-mcp',
  },
  {
    key: 'everything',
    name: 'Everything (测试)',
    command: ['npx', '-y', '@modelcontextprotocol/server-everything'],
    description: 'MCP 协议全能力测试服务器（调试用）',
    recommended: 'hot',
    popularity: '269K 次/月',
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/everything',
  },
  {
    key: 'browser-use',
    name: 'Browser Use',
    command: ['docker', 'run', '-i', '--rm', '-e', 'BROWSER_USE_LOGGING_LEVEL=info', 'ghcr.io/browser-use/browser-use:latest'],
    description: 'AI 浏览器自动化（Google 开源）：打开网页、点击、填表、截图（需 Docker）',
    recommended: 'hot',
    popularity: 'GitHub ⭐108K',
    homepage: 'https://github.com/browser-use/browser-use',
  },
  {
    key: 'firecrawl',
    name: 'Firecrawl',
    command: ['npx', '-y', 'firecrawl-mcp'],
    description: '网页爬取与结构化提取：反爬站/JS 渲染页面抓取、批量爬取、深度研究',
    recommended: 'hot',
    popularity: '412K 次/月',
    envHint: 'FIRECRAWL_API_KEY=fc-xxx',
    envDefs: [
      {
        key: 'FIRECRAWL_API_KEY',
        placeholder: 'fc-xxx',
        hint: 'Firecrawl API Key（firecrawl.dev 控制台创建，免费额度 500 次/月）',
      },
    ],
    homepage: 'https://github.com/mendableai/firecrawl-mcp-server',
  },
  {
    key: 'exa',
    name: 'Exa Search',
    command: ['npx', '-y', 'exa-mcp-server'],
    description: '语义搜索引擎：全网/代码/学术检索，返回带内容的搜索结果',
    recommended: 'popular',
    popularity: '88K 次/月',
    envHint: 'EXA_API_KEY=xxx',
    envDefs: [
      {
        key: 'EXA_API_KEY',
        placeholder: 'API Key',
        hint: 'Exa API Key（exa.ai 控制台创建）',
      },
    ],
    homepage: 'https://github.com/exa-labs/exa-mcp-server',
  },
  {
    key: 'serper',
    name: 'Serper (Google)',
    command: ['npx', '-y', 'serper-search-scrape-mcp-server'],
    description: 'Google 搜索 API（低价）：网页/图片/新闻/地点搜索 + 网页抓取',
    recommended: 'popular',
    popularity: '30K 次/月',
    envHint: 'SERPER_API_KEY=xxx',
    envDefs: [
      {
        key: 'SERPER_API_KEY',
        placeholder: 'API Key',
        hint: 'Serper API Key（serper.dev 注册免费送 2500 次）',
      },
    ],
    homepage: 'https://github.com/marcopesani/mcp-server-serper',
  },
  {
    key: 'brave-search',
    name: 'Brave Search',
    command: ['npx', '-y', '@modelcontextprotocol/server-brave-search'],
    description: 'Brave 隐私搜索引擎（官方参考服务器）',
    recommended: 'hot',
    popularity: '113K 次/月',
    envHint: 'BRAVE_API_KEY=xxx',
    envDefs: [
      {
        key: 'BRAVE_API_KEY',
        placeholder: 'API Key',
        hint: 'Brave Search API Key（brave.com/search/api 注册）',
      },
    ],
    homepage: 'https://github.com/modelcontextprotocol/servers/tree/main/src/brave-search',
  },
  {
    key: 'chrome-devtools',
    name: 'Chrome DevTools',
    command: ['npx', '-y', 'chrome-devtools-mcp'],
    description: 'Chrome 调试协议：浏览器控制、DOM 操作、性能分析（ChromeDevTools 官方）',
    recommended: 'hot',
    popularity: '7.5M 次/月',
    homepage: 'https://github.com/ChromeDevTools/chrome-devtools-mcp',
  },
  {
    key: 'notion',
    name: 'Notion',
    command: ['npx', '-y', 'notion-mcp-server'],
    description: 'Notion 页面/数据库/搜索操作（开源社区版）',
    recommended: 'popular',
    popularity: '7.7K 次/月',
    envHint: 'NOTION_TOKEN=ntn_xxx',
    envDefs: [
      {
        key: 'NOTION_TOKEN',
        placeholder: 'ntn_xxx',
        hint: 'Notion Personal Access Token（notion.so/my-integrations 创建 Integration 后复制 Token，ntn_ 开头）',
      },
    ],
    homepage: 'https://github.com/awkoy/notion-mcp-server',
  },
  {
    key: 'desktop-commander',
    name: 'Desktop Commander',
    command: ['npx', '-y', '@wonderwhy-er/desktop-commander@latest'],
    description: '本地命令执行与文件操作：终端命令、文本文件读写（无需 API Key）',
    recommended: 'hot',
    popularity: '314K 次/月',
    homepage: 'https://github.com/wonderwhy-er/DesktopCommanderMCP',
  },
  {
    key: 'npm-search',
    name: 'NPM Search',
    command: ['npx', '-y', 'npm-search-mcp-server'],
    description: 'npm 包搜索：查询包信息、版本、描述（无需 API Key）',
    homepage: 'https://modelcontextprotocol.io/',
  },
  {
    key: 'deveco-mcp',
    name: 'DevEco Toolbox',
    command: ['npx', '-y', 'deveco-mcp-server'],
    description: '鸿蒙开发工具集（open-deveco 社区）：ArkTS 语法检查 check_ets_files（基于官方 LSP）、构建、启动应用、UI 树、hilog/faultlog 日志，依赖 DevEco Studio',
    recommended: 'popular',
    envHint: 'PROJECT_PATH=<鸿蒙工程根目录>, DEVECO_PATH=<DevEco Studio 安装路径>',
    envDefs: [
      {
        key: 'PROJECT_PATH',
        placeholder: 'D:/work/harmony-project',
        hint: '鸿蒙工程根目录（含 oh-package.json5 的目录）；若 IDE 不支持 ${workspaceFolder} 需手动替换为实际路径',
      },
      {
        key: 'DEVECO_PATH',
        placeholder: 'C:/Program Files/Huawei/DevEco Studio',
        hint: 'DevEco Studio 安装路径（必填，工具箱依赖其 SDK 与内置 LSP；缺失时 check_ets_files 等工具不可用）。若启动报 "Platform package deveco-mcp-server-xxx not found"，说明 npm 镜像未同步，需在命令中加入 --registry= 参数',
      },
    ],
    homepage: 'https://github.com/open-deveco/deveco-toolbox',
  },
]

/** 根据命令字符串智能识别模板 */
export function matchMcpTemplate(command: string): McpTemplate | undefined {
  const c = command.trim().toLowerCase()
  return mcpTemplates.find((t) => t.command.some((part) => c.includes(part.toLowerCase())))
}
