/** 常用 Provider 模板库（选择模板自动填充配置）
 *  - nameKey: i18n 词条键（provider.tpl.{key}），中文界面显示中文名，模型 ID 保持英文
 *  - category: api=官方 API | coding-plan=订阅计划(Coding/Agent/Token Plan) | local=本地模型
 *  - protocol: openai(OpenAI 兼容) | anthropic(原生) | gemini(原生)
 *  覆盖国内外主流平台（2026-08-22 更新，Agent 工具调用选型，模态按官方文档核对）：
 *  国内：DeepSeek V4/Qwen3.8/GLM-5.3/Kimi K3/MiniMax M3/豆包 Seed 2.1/混元 Hy3/文心/星火/Yi/百川
 *  海外：GPT-5.6/Claude 5/Gemini 3.6/Grok 4.6/Mistral Medium 3.5/Llama 4/Nemotron 3/Cohere/Perplexity
 */
export interface ProviderTemplate {
  key: string
  name: string // 英文名（fallback）
  provider_type: string
  protocol: 'openai' | 'anthropic' | 'gemini'
  category: 'api' | 'coding-plan' | 'local'
  base_url: string
  /** 多协议端点（可选：同一厂商的 OpenAI / Anthropic / Gemini 端点，如 DeepSeek） */
  endpoints?: { protocol: string; base_url: string }[]
  keyHint: string // API Key 获取提示（显示在输入框 placeholder）
  models: string[] // 推荐默认模型（首个为默认）
  /** 视觉模型 ID 子集：选择模板时自动标记输入模态含 image（其余默认 text） */
  visionModels?: string[]
  /** 生成模型（图片/视频/音频）：模板添加时自动标记对应输出模态；endpoint 对应后端 GenEndpointStyle 标识 */
  generationModels?: {
    image?: { endpoint: string; models: string[] }
    video?: { endpoint: string; models: string[] }
    audio?: { endpoint: string; models: string[] }
  }
  free?: boolean // 有免费额度的标记
  color: string // 标识色（用于 Logo 圆点）
  matchHost: string // Base URL 智能识别用的域名特征
}

export const providerTemplates: ProviderTemplate[] = [
  // ==================== 官方 API ====================
  {
    key: 'deepseek',
    name: 'DeepSeek',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.deepseek.com',
    // DeepSeek 同时提供 OpenAI 与 Anthropic 两套端点，可在对话时按协议切换
    endpoints: [
      { protocol: 'openai', base_url: 'https://api.deepseek.com' },
      { protocol: 'anthropic', base_url: 'https://api.deepseek.com/anthropic' },
    ],
    keyHint: '在 platform.deepseek.com 申请 API Key',
    // deepseek-chat / deepseek-reasoner 已于 2026-07-24 停用，勿再收录；
    // deepseek-v4-flash-vision-exp 为视觉模型（图像理解），自动标记 image 输入模态
    models: ['deepseek-v4-pro', 'deepseek-v4-flash', 'deepseek-v4-flash-vision-exp'],
    visionModels: ['deepseek-v4-flash-vision-exp'],
    color: '#4d6bfe',
    matchHost: 'deepseek.com',
  },
  {
    key: 'openai',
    name: 'OpenAI',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.openai.com/v1',
    keyHint: '在 platform.openai.com 申请 API Key',
    // GPT-5.6 全系支持文本+图片输入（输出仅文本）
    models: ['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'],
    visionModels: ['gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'],
    // 生成模型：gpt-image-1（文生图）、gpt-4o-mini-tts（语音合成），OpenAI 兼容端点
    generationModels: {
      image: { endpoint: 'images-generations', models: ['gpt-image-1'] },
      audio: { endpoint: 'openai-speech', models: ['gpt-4o-mini-tts'] },
    },
    color: '#10a37f',
    matchHost: 'openai.com',
  },
  {
    key: 'qwen',
    name: 'Tongyi Qianwen',
    provider_type: 'aliyun',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    keyHint: '在阿里云百炼平台申请 DashScope API Key',
    // Qwen3.6+ 全系原生多模态（文本/图像/视频），qwen3-coder-plus 同样支持图像输入
    models: ['qwen3.8-max', 'qwen3.7-plus', 'qwen3.6-flash', 'qwen3-coder-plus'],
    visionModels: ['qwen3.8-max', 'qwen3.7-plus', 'qwen3.6-flash', 'qwen3-coder-plus'],
    free: true,
    color: '#615ced',
    matchHost: 'dashscope.aliyuncs.com',
  },
  {
    key: 'kimi',
    name: 'Kimi (Moonshot)',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.moonshot.cn/v1',
    keyHint: '在 platform.moonshot.cn 申请 API Key',
    // Kimi K2.5+ 全系原生多模态（文本/图像/文档），K3 支持视频理解
    models: ['kimi-k3', 'kimi-k2.6', 'kimi-k2.5'],
    visionModels: ['kimi-k3', 'kimi-k2.6', 'kimi-k2.5'],
    color: '#1e1e1e',
    matchHost: 'moonshot.cn',
  },
  {
    key: 'zhipu',
    name: 'Zhipu GLM',
    provider_type: 'zhipu',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://open.bigmodel.cn/api/paas/v4',
    keyHint: '在 open.bigmodel.cn 申请 API Key（glm-4-flash 免费）',
    // GLM 主系（5.2/5/4.7/4-flash）为纯文本；视觉能力在 glm-4.6v（图片理解，-V 产品线）
    models: ['glm-5.2', 'glm-5', 'glm-4.7', 'glm-4.6v', 'glm-4-flash'],
    visionModels: ['glm-4.6v'],
    // 生成模型：cogview-3-flash（文生图）、cogvideox-flash（视频异步任务）
    generationModels: {
      image: { endpoint: 'images-generations', models: ['cogview-3-flash'] },
      video: { endpoint: 'zhipu-video', models: ['cogvideox-flash'] },
    },
    free: true,
    color: '#3859ff',
    matchHost: 'bigmodel.cn',
  },
  {
    key: 'doubao',
    name: 'Doubao (Volcengine)',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://ark.cn-beijing.volces.com/api/v3',
    keyHint: '在火山引擎方舟控制台创建推理接入点（模型名填接入点 ID 或模型家族名）',
    // doubao-seed-code-preview 已于 2025-11 下线，改用官方推荐的 doubao-seed-evolving（周级迭代模型）
    // Doubao-Seed 2.x 全系支持文本+图片输入
    models: ['doubao-seed-2.1-pro', 'doubao-seed-2.1-turbo', 'doubao-seed-2.0-pro', 'doubao-seed-evolving'],
    visionModels: ['doubao-seed-2.1-pro', 'doubao-seed-2.1-turbo', 'doubao-seed-2.0-pro', 'doubao-seed-evolving'],
    // 生成模型：doubao-seedream（文生图）、doubao-seedance（视频，方舟异步任务）；ID 以方舟控制台最新为准
    generationModels: {
      image: { endpoint: 'images-generations', models: ['doubao-seedream-4-0-250828'] },
      video: { endpoint: 'ark-video', models: ['doubao-seedance-2-0-260128'] },
    },
    color: '#3370ff',
    matchHost: 'volces.com',
  },
  {
    key: 'minimax',
    name: 'MiniMax (International)',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.minimax.io/v1',
    keyHint: '在 platform.minimax.io 申请 API Key（国际版）',
    // 仅 MiniMax-M3 原生多模态（文本/图片/视频/音频）；M2.7/M2.5 为纯文本
    models: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.5'],
    visionModels: ['MiniMax-M3'],
    // 生成模型：image-01（文生图）、MiniMax-H3（视频异步任务）、speech-2.8-hd（语音合成）
    generationModels: {
      image: { endpoint: 'minimax-image', models: ['image-01'] },
      video: { endpoint: 'minimax-video', models: ['MiniMax-H3'] },
      audio: { endpoint: 'minimax-t2a', models: ['speech-2.8-hd'] },
    },
    color: '#6d28d9',
    matchHost: 'minimax.io',
  },
  {
    key: 'minimax-cn',
    name: 'MiniMax (China)',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.minimaxi.com/v1',
    keyHint: '在 platform.minimaxi.com 申请 API Key（国内版）',
    // 仅 MiniMax-M3 原生多模态；M2.7/M2.5 为纯文本
    models: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.5'],
    visionModels: ['MiniMax-M3'],
    // 生成模型：image-01（文生图）、MiniMax-H3（视频异步任务）、speech-2.8-hd（语音合成）
    generationModels: {
      image: { endpoint: 'minimax-image', models: ['image-01'] },
      video: { endpoint: 'minimax-video', models: ['MiniMax-H3'] },
      audio: { endpoint: 'minimax-t2a', models: ['speech-2.8-hd'] },
    },
    color: '#7c3aed',
    matchHost: 'minimaxi.com',
  },
  {
    key: 'xiaomi-mimo',
    name: 'Xiaomi MiMo',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.xiaomimimo.com/v1',
    keyHint: '在 platform.xiaomimimo.com 申请 API Key（sk-，1M 上下文）',
    // mimo-v2.5 原生全模态（文本/图片/视频/音频）；v2.5-pro 为纯文本长程 Agent 旗舰
    models: ['mimo-v2.5-pro', 'mimo-v2.5'],
    visionModels: ['mimo-v2.5'],
    free: true,
    color: '#ff6900',
    matchHost: 'xiaomimimo.com',
  },
  {
    key: 'siliconflow',
    name: 'SiliconFlow',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.siliconflow.cn/v1',
    keyHint: '在 cloud.siliconflow.cn 申请 API Key（有免费模型）',
    models: ['deepseek-ai/DeepSeek-V3.2', 'Qwen/Qwen3-235B-A22B', 'zai-org/GLM-5', 'zai-org/GLM-4.5V'],
    visionModels: ['zai-org/GLM-4.5V'],
    // 生成模型：FLUX 系列文生图（OpenAI 兼容 images/generations）
    generationModels: {
      image: { endpoint: 'images-generations', models: ['black-forest-labs/flux-schnell', 'black-forest-labs/FLUX.1-dev'] },
    },
    free: true,
    color: '#2b6de8',
    matchHost: 'siliconflow.cn',
  },
  {
    key: 'openrouter',
    name: 'OpenRouter',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://openrouter.ai/api/v1',
    keyHint: '在 openrouter.ai 申请 API Key（聚合多家模型）',
    models: ['openai/gpt-5.6-sol', 'anthropic/claude-opus-5', 'google/gemini-3.5-flash', 'deepseek/deepseek-v4-flash'],
    visionModels: ['openai/gpt-5.6-sol', 'anthropic/claude-opus-5', 'google/gemini-3.5-flash'],
    // 生成模型：gpt-image-1（文生图，OpenAI 兼容端点）
    generationModels: {
      image: { endpoint: 'images-generations', models: ['openai/gpt-image-1'] },
    },
    color: '#7c3aed',
    matchHost: 'openrouter.ai',
  },
  {
    key: 'anthropic',
    name: 'Anthropic Claude',
    provider_type: 'anthropic',
    protocol: 'anthropic',
    category: 'api',
    base_url: 'https://api.anthropic.com',
    keyHint: '在 console.anthropic.com 申请 API Key（原生 Anthropic 协议）',
    // Claude 全系支持文本+图片输入
    models: ['claude-opus-5', 'claude-fable-5', 'claude-sonnet-5', 'claude-opus-4-8', 'claude-haiku-4-5'],
    visionModels: ['claude-opus-5', 'claude-fable-5', 'claude-sonnet-5', 'claude-opus-4-8', 'claude-haiku-4-5'],
    color: '#d97757',
    matchHost: 'anthropic.com',
  },
  {
    key: 'gemini',
    name: 'Google Gemini',
    provider_type: 'gemini',
    protocol: 'gemini',
    category: 'api',
    base_url: 'https://generativelanguage.googleapis.com',
    keyHint: '在 aistudio.google.com 申请 API Key（原生 Gemini 协议）',
    // gemini-3.5-pro / gemini-3-pro 尚未 GA，旗舰为 gemini-3.6-flash（2026-07 稳定）
    // Gemini 全系原生多模态（文本/图像/音频/视频/PDF）
    models: ['gemini-3.6-flash', 'gemini-3.5-flash', 'gemini-3.1-pro-preview'],
    visionModels: ['gemini-3.6-flash', 'gemini-3.5-flash', 'gemini-3.1-pro-preview'],
    free: true,
    color: '#4285f4',
    matchHost: 'generativelanguage.googleapis.com',
  },
  {
    key: 'grok',
    name: 'xAI Grok',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.x.ai/v1',
    keyHint: '在 console.x.ai 申请 API Key（Grok 4.6 支持 Agent 工具调用，500K 上下文）',
    // Grok 4.x 全系支持文本+图片输入
    models: ['grok-4-6', 'grok-4-5'],
    visionModels: ['grok-4-6', 'grok-4-5'],
    color: '#161616',
    matchHost: 'x.ai',
  },
  {
    key: 'mistral',
    name: 'Mistral AI',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.mistral.ai/v1',
    keyHint: '在 console.mistral.ai 申请 API Key（-latest 别名已弃用，用显式版本 ID）',
    // 仅 mistral-medium-3-5 带视觉编码器（图片理解）；Large/Small 系列为纯文本
    models: ['mistral-medium-3-5', 'mistral-large-2512', 'mistral-small-2603'],
    visionModels: ['mistral-medium-3-5'],
    color: '#f7b500',
    matchHost: 'mistral.ai',
  },
  {
    key: 'perplexity',
    name: 'Perplexity',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.perplexity.ai',
    keyHint: '在 perplexity.ai/settings/api 申请 API Key',
    // Sonar 系列均支持图片输入（sonar-pro 为旗舰）
    models: ['sonar-pro', 'sonar-reasoning-pro', 'sonar'],
    visionModels: ['sonar-pro', 'sonar-reasoning-pro', 'sonar'],
    color: '#20808d',
    matchHost: 'perplexity.ai',
  },
  {
    key: 'groq',
    name: 'Groq',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.groq.com/openai/v1',
    keyHint: '在 console.groq.com 申请 API Key（推理极快）',
    models: ['openai/gpt-oss-120b', 'qwen/qwen3.6-27b'],
    free: true,
    color: '#f55036',
    matchHost: 'groq.com',
  },
  {
    key: 'iflytek',
    name: 'iFlytek Spark',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://spark-api-open.xf-yun.com/v1',
    keyHint: '在 xfyun.cn 控制台申请星火 API Key',
    // OpenAI 兼容端点 model 取值：4.0Ultra / generalv3.5 / max-32k / generalv3 / pro-128k / lite（无 spark-x）
    models: ['4.0Ultra', 'pro-128k'],
    color: '#155eef',
    matchHost: 'xf-yun.com',
  },
  {
    key: 'baidu-ernie',
    name: 'Baidu ERNIE',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://qianfan.baidubce.com/v2',
    keyHint: '在千帆 ModelBuilder 控制台申请 API Key',
    // 视觉走 ernie-4.5-turbo-vl（图片理解）；-128k/-32k 为文本接入点
    models: ['ernie-4.5-turbo-vl', 'ernie-4.5-turbo-128k', 'ernie-4.5-turbo-32k'],
    visionModels: ['ernie-4.5-turbo-vl'],
    color: '#2932e1',
    matchHost: 'qianfan.baidubce.com',
  },
  {
    key: 'hunyuan',
    name: 'Tencent Hunyuan',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.hunyuan.cloud.tencent.com/v1',
    keyHint: '在腾讯云 TokenHub 开通混元 Hy3（旧版 hunyuan-t1/turbos 已 2026-06 下线）',
    models: ['hy3', 'hy3-preview'],
    color: '#0052d9',
    matchHost: 'hunyuan.cloud.tencent.com',
  },
  {
    key: 'stepfun',
    name: 'StepFun',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.stepfun.com/v1',
    keyHint: '在 platform.stepfun.com 申请 API Key',
    // step-3.7-flash 为官方推荐多模态旗舰（原生图片/视频理解，256K 上下文）
    models: ['step-3.7-flash', 'step-2-mini', 'step-2-16k'],
    visionModels: ['step-3.7-flash'],
    color: '#8b5cf6',
    matchHost: 'stepfun.com',
  },
  {
    key: '01ai',
    name: '01.AI Yi',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.lingyiwanwu.com/v1',
    keyHint: '在 platform.lingyiwanwu.com 申请 API Key',
    models: ['yi-lightning'],
    color: '#0f0f0f',
    matchHost: 'lingyiwanwu.com',
  },
  {
    key: 'baichuan',
    name: 'Baichuan',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.baichuan-ai.com/v1',
    keyHint: '在 platform.baichuan-ai.com 申请 API Key',
    // Baichuan4 原生多模态（文本/图片/视频/音频输入）
    models: ['Baichuan4'],
    visionModels: ['Baichuan4'],
    color: '#0052d9',
    matchHost: 'baichuan-ai.com',
  },
  {
    key: 'jina',
    name: 'Jina AI',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.jina.ai/v1',
    keyHint: '在 jina.ai 申请 API Key',
    // jina-embeddings-v4 为向量模型（不支持 Chat Completions），勿放入模板
    models: ['jina-deepsearch-v1'],
    color: '#d53369',
    matchHost: 'jina.ai',
  },
  {
    key: 'together',
    name: 'Together AI',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.together.ai/v1',
    keyHint: '在 api.together.ai 申请 API Key（开源模型托管，旧域名 api.together.xyz 已弃用）',
    models: ['meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8', 'Qwen/Qwen3-235B-A22B-Instruct-2507'],
    visionModels: ['meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8'],
    color: '#7c3aed',
    matchHost: 'together.ai',
  },
  {
    key: 'cohere',
    name: 'Cohere',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    // OpenAI 兼容端点（compatibility/v1），原生 v2 端点不兼容 OpenAI SDK
    base_url: 'https://api.cohere.ai/compatibility/v1',
    keyHint: '在 dashboard.cohere.com 申请 API Key（OpenAI 兼容端点）',
    models: ['command-a-plus-05-2026', 'command-a-03-2025'],
    color: '#39594d',
    matchHost: 'cohere.com',
  },
  {
    key: 'fireworks',
    name: 'Fireworks AI',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://api.fireworks.ai/inference/v1',
    keyHint: '在 fireworks.ai 申请 API Key（开源模型高速推理）',
    models: ['accounts/fireworks/models/deepseek-v3p1', 'accounts/fireworks/models/llama-v4-maverick'],
    visionModels: ['accounts/fireworks/models/llama-v4-maverick'],
    free: true,
    color: '#f97316',
    matchHost: 'fireworks.ai',
  },
  {
    key: 'nvidia-nim',
    name: 'NVIDIA NIM',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'api',
    base_url: 'https://integrate.api.nvidia.com/v1',
    keyHint: '在 build.nvidia.com 申请 NVIDIA API Key（免费额度）',
    models: ['nvidia/nemotron-3-ultra-550b-a55b', 'meta/llama-4-maverick-17b-128e-instruct'],
    visionModels: ['meta/llama-4-maverick-17b-128e-instruct'],
    free: true,
    color: '#76b900',
    matchHost: 'api.nvidia.com',
  },

  // ==================== Coding / Agent / Token Plan（订阅计划） ====================
  {
    key: 'minimax-coding-plan',
    name: 'MiniMax Coding Plan',
    provider_type: 'openai-compatible',
    protocol: 'anthropic',
    category: 'coding-plan',
    // Coding Plan 走国内版域名 api.minimaxi.com（国际版 minimax.io 为按量 API，不消耗套餐额度）
    base_url: 'https://api.minimaxi.com/anthropic',
    keyHint: '订阅 MiniMax Coding Plan 后，在官方渠道获取 Coding Plan Key',
    models: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.7-highspeed'],
    visionModels: ['MiniMax-M3'],
    color: '#6d28d9',
    matchHost: 'minimaxi.com/anthropic',
  },
  {
    key: 'volcengine-coding-plan',
    name: 'Volcengine Coding/Agent Plan',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'coding-plan',
    // Coding Plan 必须走套餐专用端点，普通 /api/v3 会按量计费额外扣费
    base_url: 'https://ark.cn-beijing.volces.com/api/coding/v3',
    keyHint: '订阅火山方舟 Coding/Agent Plan 获取 Key（ark-code-latest 自动调度）',
    // 套餐内支持视觉的模型（ark-code-latest 为自动调度，不预标记）
    models: ['ark-code-latest', 'doubao-seed-2.1-turbo', 'kimi-k2.7-code', 'glm-5.3', 'minimax-m3', 'deepseek-v4-pro'],
    visionModels: ['doubao-seed-2.1-turbo', 'minimax-m3'],
    color: '#3370ff',
    matchHost: 'volces.com',
  },
  {
    key: 'zhipu-coding-plan',
    name: 'Zhipu GLM Coding Plan',
    provider_type: 'zhipu',
    protocol: 'anthropic',
    category: 'coding-plan',
    base_url: 'https://open.bigmodel.cn/api/anthropic',
    keyHint: '订阅智谱 GLM Coding Plan 获取 Key（Anthropic 兼容端点，glm-5.3 已上线）',
    models: ['glm-5.3', 'glm-5-turbo', 'glm-4.7'],
    color: '#3859ff',
    matchHost: 'bigmodel.cn/api/anthropic',
  },
  {
    key: 'kimi-coding-plan',
    name: 'Kimi Coding Plan',
    provider_type: 'openai-compatible',
    protocol: 'anthropic',
    category: 'coding-plan',
    base_url: 'https://api.moonshot.cn/anthropic',
    keyHint: '订阅 Kimi Coding Plan 获取 Key（Anthropic 兼容端点）',
    // kimi-for-coding 系列为代码专用模型（纯文本）；kimi-k3 支持图片输入
    models: ['kimi-for-coding', 'kimi-k3', 'kimi-for-coding-highspeed'],
    visionModels: ['kimi-k3'],
    color: '#1e1e1e',
    matchHost: 'moonshot.cn/anthropic',
  },
  {
    key: 'aliyun-coding-plan',
    name: 'Aliyun Bailian Token Plan',
    provider_type: 'aliyun',
    protocol: 'anthropic',
    category: 'coding-plan',
    // Token Plan 必须走套餐专用域名，普通 dashscope.aliyuncs.com 是按量付费端点
    base_url: 'https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic',
    keyHint: '订阅百炼 Token Plan（原 Coding Plan）获取 Key（套餐专用域名）',
    models: ['qwen3.8-max', 'qwen3.7-plus', 'glm-5.3'],
    visionModels: ['qwen3.8-max', 'qwen3.7-plus'],
    color: '#615ced',
    matchHost: 'token-plan.cn-beijing.maas.aliyuncs.com',
  },
  {
    key: 'xiaomi-token-plan',
    name: 'Xiaomi MiMo Token Plan',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'coding-plan',
    base_url: 'https://token-plan-cn.xiaomimimo.com/v1',
    keyHint: '订阅 MiMo Token Plan 获取区域端点与 tp- 开头 Key（cn/sgp/ams）',
    models: ['mimo-v2.5-pro', 'mimo-v2.5'],
    visionModels: ['mimo-v2.5'],
    color: '#ff6900',
    matchHost: 'token-plan',
  },
  {
    key: 'github-copilot',
    name: 'GitHub Copilot',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'coding-plan',
    // 请求端点为 {base}/chat/completions（勿把路径写进 base_url，否则会重复拼接）
    base_url: 'https://api.githubcopilot.com',
    keyHint: '使用 GitHub Copilot 订阅的 OAuth Token（gh auth token）',
    // Copilot 套餐内模型全系支持图片输入
    models: ['gpt-5.6-sol', 'claude-sonnet-5', 'gemini-3.5-flash'],
    visionModels: ['gpt-5.6-sol', 'claude-sonnet-5', 'gemini-3.5-flash'],
    color: '#24292f',
    matchHost: 'githubcopilot.com',
  },
  {
    key: 'cursor',
    name: 'Cursor',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'coding-plan',
    base_url: 'https://api2.cursor.sh/v1',
    keyHint: '使用 Cursor Pro 订阅的会话 Token（.cursor 目录中的 token，非官方公开端点）',
    models: ['gpt-5.4', 'claude-sonnet-4.6'],
    visionModels: ['gpt-5.4', 'claude-sonnet-4.6'],
    color: '#7c3aed',
    matchHost: 'cursor.sh',
  },
  {
    key: 'windsurf',
    name: 'Windsurf',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'coding-plan',
    base_url: 'https://api.windsurf.com/v1',
    keyHint: '使用 Windsurf 订阅的 Token（非官方公开端点）',
    models: ['gpt-5.4', 'claude-sonnet-4.6'],
    visionModels: ['gpt-5.4', 'claude-sonnet-4.6'],
    color: '#0ea5e9',
    matchHost: 'windsurf.com',
  },

  // ==================== 本地模型 ====================
  {
    key: 'ollama',
    name: 'Ollama',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'local',
    base_url: 'http://localhost:11434/v1',
    keyHint: '本地模型无需 Key，先运行 ollama serve',
    // qwen3-vl:8b 为 Ollama 视觉模型（qwen3-vl 系列）
    models: ['qwen3:8b', 'qwen2.5:7b', 'llama3.1:8b', 'qwen3-vl:8b'],
    visionModels: ['qwen3-vl:8b'],
    free: true,
    color: '#7d8796',
    matchHost: 'localhost:11434',
  },
  {
    key: 'lmstudio',
    name: 'LM Studio',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    category: 'local',
    base_url: 'http://localhost:1234/v1',
    keyHint: '本地模型无需 Key，先在 LM Studio 中加载模型并开启本地服务',
    models: ['qwen3-235b-a22b'],
    free: true,
    color: '#6366f1',
    matchHost: 'localhost:1234',
  },
]

/** 模板分类元信息（顺序即展示顺序） */
export const templateCategories: { key: ProviderTemplate['category']; labelKey: string }[] = [
  { key: 'api', labelKey: 'provider.catApi' },
  { key: 'coding-plan', labelKey: 'provider.catPlan' },
  { key: 'local', labelKey: 'provider.catLocal' },
]

/** 根据 Base URL 智能识别模板（返回匹配项） */
export function matchTemplateByUrl(url: string): ProviderTemplate | undefined {
  const trimmed = url.trim().toLowerCase()
  if (!trimmed) return undefined
  return providerTemplates.find((t) => trimmed.includes(t.matchHost.toLowerCase()))
}
