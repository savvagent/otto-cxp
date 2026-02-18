package com.cxp.sdk

/**
 * CXP SDK for Android — stub implementation (v2).
 * Connect to the CXP MCP server via streamable HTTP transport.
 */
class CxpClient(private val baseUrl: String) {

    // CDP

    suspend fun getCustomer(id: String): Map<String, Any?> {
        TODO("Not yet implemented — coming in v2")
    }

    suspend fun searchCustomers(query: String): List<Map<String, Any?>> {
        TODO("Not yet implemented — coming in v2")
    }

    suspend fun getCustomer360(id: String): Map<String, Any?> {
        TODO("Not yet implemented — coming in v2")
    }

    // Support

    suspend fun createTicket(customerId: String, subject: String): Map<String, Any?> {
        TODO("Not yet implemented — coming in v2")
    }

    suspend fun listTickets(customerId: String? = null): List<Map<String, Any?>> {
        TODO("Not yet implemented — coming in v2")
    }

    // Feedback

    suspend fun submitFeedback(
        customerId: String,
        feedbackType: String,
        body: String? = null,
        rating: Int? = null,
    ): Map<String, Any?> {
        TODO("Not yet implemented — coming in v2")
    }

    suspend fun calculateNps(surveyId: String): Double {
        TODO("Not yet implemented — coming in v2")
    }
}
