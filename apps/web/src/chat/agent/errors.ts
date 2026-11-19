/** What an llmApi error code means to the person chatting. */
const TEXT: Record<string, [en: string, zh: string]> = {
  UPSTREAM_AUTH: ['The model service rejected this key. Check it and save it again.', '模型服务拒绝了这个 key，请检查后重新保存。'],
  MODEL_NOT_FOUND: ['The model service has no such model. Pick another one.', '模型服务没有这个模型，请换一个。'],
  UPSTREAM_RATE_LIMITED: ['The model service is busy or limiting requests. Try again shortly.', '模型服务繁忙或限流，请稍后再试。'],
  UPSTREAM_QUOTA: ['The model service account is out of credit.', '模型服务账户余额或额度不足。'],
  UPSTREAM_REJECTED: ['The model service refused this request. The chat may be too long.', '模型服务拒绝了这个请求，可能是对话太长。'],
  RATE_LIMITED: ['You are sending too fast. Wait a moment and try again.', '发送得太快了，请稍等片刻再试。'],
  TIMEOUT: ['The model service took too long to answer.', '模型服务响应超时。'],
  NETWORK: ['The connection was lost.', '网络连接中断。'],
  INTERRUPTED: ['The answer was cut off.', '回答意外中断。'],
  PROVIDER_NOT_FOUND: ['This model service was removed. Choose another.', '这个模型服务已被删除，请换一个。'],
  PROVIDER_NOT_WEB_USABLE: ['This model service works in the desktop app only.', '这个模型服务只能在桌面端使用。'],
  EGRESS_DENIED: ['This service address cannot be reached from LingXi.', '灵犀无法访问这个服务地址。'],
  INVALID_SESSION: ['Your sign-in has expired. Sign in again.', '登录已过期，请重新登录。'],
  SIGN_IN_REQUIRED: ['Your sign-in has expired. Sign in again.', '登录已过期，请重新登录。'],
  PROVIDER_LIMIT: ['You can keep up to 20 model services.', '最多可以保存 20 个模型服务。'],
  REVISION_CONFLICT: ['This service changed on another device. Reload and try again.', '这个服务已在其他设备上修改，请刷新后重试。'],
  INVALID_INPUT: ['Some of this is not valid. Check the fields and try again.', '有内容不符合要求，请检查后重试。'],
  MODEL_REQUIRED: ['Add a model to this service first.', '请先为这个服务添加一个模型。'],
};
export function errorText(code: string | undefined, zh: boolean): string {
  const entry = TEXT[code ?? ''];
  return entry ? entry[zh ? 1 : 0] : zh ? '模型服务暂时不可用，请稍后再试。' : 'The model service is unavailable right now. Try again later.';
}
