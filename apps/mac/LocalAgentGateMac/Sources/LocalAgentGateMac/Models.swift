import Foundation

struct PendingRequest: Codable, Identifiable {
    struct AgentInfo: Codable {
        let id: String
        let name: String
        let sessionId: String?
    }
    struct ProjectInfo: Codable {
        let path: String
        let name: String
        let gitRemote: String?
        let gitBranch: String?
    }
    struct ActionInfo: Codable {
        let kind: String
        let command: String
        let argv: [String]
        let workingDirectory: String
    }
    struct RiskInfo: Codable {
        let level: String
        let reasons: [String]
        let matchedRules: [String]
    }
    struct PolicyInfo: Codable {
        let decision: String
        let matchedRuleIds: [String]
    }

    let id: String
    let createdAt: String
    let expiresAt: String
    let agent: AgentInfo
    let project: ProjectInfo
    let action: ActionInfo
    let risk: RiskInfo
    let policy: PolicyInfo
}
