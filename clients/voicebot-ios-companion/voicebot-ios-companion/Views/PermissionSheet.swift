//
//  PermissionSheet.swift
//  voicebot-ios-companion
//
//  Agent permission modal: options send ACP option_id via POST /control/permission.
//

import SwiftUI

struct PermissionSheet: View {
    @EnvironmentObject var vm: CompanionViewModel
    let request: PermissionRequest

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                header

                Text(request.description)
                    .font(.body)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(Color(.secondarySystemBackground))
                    .cornerRadius(10)
                    .accessibilityIdentifier("permissionSheetDescription")
                    .textSelection(.enabled)

                if let err = vm.permissionResolveError {
                    Text(err)
                        .font(.caption)
                        .foregroundColor(.red)
                        .accessibilityIdentifier("permissionSheetError")
                }

                VStack(spacing: 10) {
                    ForEach(request.options) { opt in
                        Button {
                            vm.resolvePermission(optionId: opt.id)
                        } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(primaryLabel(opt))
                                        .font(.headline)
                                    if let kind = opt.kind, !kind.isEmpty {
                                        Text(kindLabel(kind))
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                    Text("id: \(opt.id)")
                                        .font(.caption2.monospaced())
                                        .foregroundColor(.secondary.opacity(0.8))
                                }
                                Spacer()
                                if vm.isResolvingPermission {
                                    ProgressView()
                                } else {
                                    Image(systemName: icon(for: opt))
                                }
                            }
                            .padding()
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(buttonBackground(for: opt))
                            .foregroundColor(buttonForeground(for: opt))
                            .cornerRadius(12)
                        }
                        .buttonStyle(.plain)
                        .disabled(vm.isResolvingPermission)
                        .accessibilityIdentifier("permissionSheetOption-\(opt.id)")
                        .accessibilityLabel("\(primaryLabel(opt)), option \(opt.id)")
                    }
                }

                Text("Voice answers on the host still work while this sheet is open.")
                    .font(.caption2)
                    .foregroundColor(.secondary)

                Spacer(minLength: 0)
            }
            .padding()
            .navigationTitle("Agent permission")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Later") {
                        // Keep pendingPermission so StatusBar chip can reopen the sheet.
                        vm.permissionSheetDismissedByUser = true
                    }
                    .disabled(vm.isResolvingPermission)
                    .accessibilityIdentifier("permissionSheetLater")
                }
            }
            .interactiveDismissDisabled(vm.isResolvingPermission)
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "hand.raised.fill")
                .font(.title2)
                .foregroundColor(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text(request.agentName.isEmpty ? "Agent" : request.agentName)
                    .font(.headline)
                Text("Task \(request.taskId)")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
            Spacer()
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Permission request from \(request.agentName), task \(request.taskId)")
    }

    private func primaryLabel(_ opt: PermissionOption) -> String {
        if opt.label.isEmpty || opt.label == opt.id {
            return opt.id
        }
        return opt.label
    }

    private func kindLabel(_ kind: String) -> String {
        switch kind.lowercased() {
        case "allow": return "Allow this action"
        case "reject", "deny": return "Deny this action"
        default: return kind
        }
    }

    private func icon(for opt: PermissionOption) -> String {
        let k = (opt.kind ?? opt.id).lowercased()
        if k.contains("allow") { return "checkmark.circle.fill" }
        if k.contains("deny") || k.contains("reject") { return "xmark.circle.fill" }
        return "circle"
    }

    private func buttonBackground(for opt: PermissionOption) -> Color {
        let k = (opt.kind ?? opt.id).lowercased()
        if k.contains("allow") { return Color.green.opacity(0.15) }
        if k.contains("deny") || k.contains("reject") { return Color.red.opacity(0.12) }
        return Color(.tertiarySystemBackground)
    }

    private func buttonForeground(for opt: PermissionOption) -> Color {
        let k = (opt.kind ?? opt.id).lowercased()
        if k.contains("allow") { return .green }
        if k.contains("deny") || k.contains("reject") { return .red }
        return .primary
    }
}

#Preview {
    PermissionSheet(
        request: PermissionRequest(
            taskId: "t1",
            agentName: "hermes",
            description: "bash: ls -la /tmp",
            options: [
                PermissionOption(id: "allow", label: "Allow once", kind: "allow"),
                PermissionOption(id: "deny", label: "Deny", kind: "reject"),
                PermissionOption(id: "always_allow", label: "Always allow", kind: "allow"),
            ]
        )
    )
    .environmentObject(CompanionViewModel())
}
