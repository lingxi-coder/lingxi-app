package com.lingxi.code

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.FfiConverterTypeSessionAgentSummaryDto
import com.lingxi.code.bindings.client.SessionAgentSummaryDto
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/** Exercises the generated converter and the linked native RustBuffer allocator on device. */
@RunWith(AndroidJUnit4::class)
class ReportDeliveryBindingsTest {
    @Test fun optionalDeliveryRoundTripsWithoutShiftingOtherAgentFields() {
        for (delivery in listOf(null, "pending", "delivered", "flagged", "withheld")) {
            val agent = SessionAgentSummaryDto(
                reportDelivery = delivery,
                agentId = "agent:binding-test",
                name = ClientTextDto("测试 agent".map { it.code.toUShort() }),
                agentType = "explorer",
                model = "subscription-model",
                modelProfile = "openai-chatgpt",
                status = "completed",
                latestActivity = ClientTextDto("Done".map { it.code.toUShort() }),
                updatedAtMs = 123456789UL,
            )
            val buffer = FfiConverterTypeSessionAgentSummaryDto.lower(agent)
            assertEquals(agent, FfiConverterTypeSessionAgentSummaryDto.lift(buffer))
        }
    }
}
