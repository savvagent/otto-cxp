import Foundation

/// CXP SDK for iOS — stub implementation (v2).
/// Connect to the CXP MCP server via streamable HTTP transport.
public class CxpClient {
    private let baseURL: URL

    public init(baseURL: URL) {
        self.baseURL = baseURL
    }

    // MARK: - CDP

    public func getCustomer(id: String) async throws -> [String: Any] {
        fatalError("Not yet implemented — coming in v2")
    }

    public func searchCustomers(query: String) async throws -> [[String: Any]] {
        fatalError("Not yet implemented — coming in v2")
    }

    public func getCustomer360(id: String) async throws -> [String: Any] {
        fatalError("Not yet implemented — coming in v2")
    }

    // MARK: - Support

    public func createTicket(customerId: String, subject: String) async throws -> [String: Any] {
        fatalError("Not yet implemented — coming in v2")
    }

    public func listTickets(customerId: String? = nil) async throws -> [[String: Any]] {
        fatalError("Not yet implemented — coming in v2")
    }

    // MARK: - Feedback

    public func submitFeedback(customerId: String, feedbackType: String, body: String? = nil, rating: Int? = nil) async throws -> [String: Any] {
        fatalError("Not yet implemented — coming in v2")
    }

    public func calculateNps(surveyId: String) async throws -> Double {
        fatalError("Not yet implemented — coming in v2")
    }
}
