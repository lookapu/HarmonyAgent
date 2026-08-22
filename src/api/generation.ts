import { invokeWithError } from './invoke'

/** 生成媒体类型：image | video | audio */
export type GenKind = 'image' | 'video' | 'audio'

/**
 * 生成媒体（图片/视频/音频）：走对应服务商的生成模型（按 output_modalities 路由）。
 * 结果以 assistant 消息（媒体标记 content）入库，完成后后端推送 gen-done 事件。
 * @param conversationId 目标会话
 * @param kind 生成类型
 * @param prompt 生成描述
 * @param modelId 指定生成模型记录 ID（缺省按当前激活 Provider 自动路由）
 * @param images 参考图（data URL；视频生图参考帧）
 */
export const generateMedia = (
  conversationId: string,
  kind: GenKind,
  prompt: string,
  modelId?: string,
  images?: string[],
) =>
  invokeWithError<void>('generate_media', {
    conversationId,
    kind,
    prompt,
    modelId: modelId ?? null,
    images: images ?? null,
  })
