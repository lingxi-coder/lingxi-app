package com.lingxi.code.apps

/** User-reviewable draft for both creation and continued development. */
internal object LocalAppAuthoringPrompt {
    private const val agentRecoveryGuidance = "如果应用使用 Agent，请在提交前持久保存可恢复的应用状态及待处理的 runId、threadId；重新打开时读取宿主任务状态和事件或聊天历史，以只读方式重连同一 run 和 thread。请求结果不确定或任务中断时，不要自动提交、重发或重放工具；仅在宿主确认时显示正在运行；明确显示中断或未知状态和用户需要采取的操作。用户检查可能已发生的副作用后，新的尝试必须由用户明确发起并使用新的 runId；只有宿主终止结果支持时才显示完成。iOS 可能暂停或终止后台工作，不要承诺无限期运行。"

    fun compose(name: String, appId: String?, existingDraft: String): String {
        if (appId != null && existingDraft.contains(agentRecoveryGuidance)) return existingDraft
        val intro = if (appId == null) {
            "请在这个源码项目中创建应用：$name。使用 local-app skill 和 LocalApp schema 2，完成后发布到本机应用库。"
        } else {
            "请在已绑定的源码项目中继续开发应用：$name（LocalApp id: $appId）。使用 local-app skill，在同一个 id 下发布修改。"
        }
        val prompt = "$intro\n$agentRecoveryGuidance"
        return if (existingDraft.isEmpty()) "$prompt\n需求：\n" else "$prompt\n\n$existingDraft"
    }
}
