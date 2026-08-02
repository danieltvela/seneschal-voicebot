//
//  TimelineView.swift
//  voicebot-ios-companion
//
//  Compact rows for tool / system / agent / error / MCP events.
//

import SwiftUI

struct TimelineView: View {
    @EnvironmentObject var vm: CompanionViewModel
    @State private var expandedIds: Set<String> = []

    var body: some View {
        Group {
            if vm.timeline.isEmpty {
                emptyState
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 8) {
                            ForEach(vm.timeline) { item in
                                TimelineRow(
                                    item: item,
                                    expanded: expandedIds.contains(item.id),
                                    onToggle: {
                                        if expandedIds.contains(item.id) {
                                            expandedIds.remove(item.id)
                                        } else {
                                            expandedIds.insert(item.id)
                                        }
                                    }
                                )
                                .id(item.id)
                            }
                        }
                        .padding()
                    }
                    .accessibilityIdentifier("timelineList")
                    .accessibilityLabel("Event timeline")
                    .onChange(of: vm.timeline.count) { _ in
                        if let last = vm.timeline.last {
                            withAnimation {
                                proxy.scrollTo(last.id, anchor: .bottom)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Timeline")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                if !vm.timeline.isEmpty {
                    Button("Clear") {
                        vm.clearTimeline()
                    }
                    .accessibilityIdentifier("clearTimelineButton")
                    .accessibilityLabel("Clear timeline")
                    .accessibilityHint("Removes all timeline events from this session")
                }
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 40))
                .foregroundColor(.secondary.opacity(0.5))
                .accessibilityHidden(true)
            Text("No events yet")
                .font(.title3.weight(.semibold))
                .foregroundColor(.secondary)
            Text("Tool calls, system notices, and agent activity appear here")
                .font(.body)
                .foregroundColor(.secondary.opacity(0.85))
                .multilineTextAlignment(.center)
                .padding(.horizontal)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityIdentifier("timelineEmpty")
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Row

private struct TimelineRow: View {
    let item: TimelineItem
    let expanded: Bool
    let onToggle: () -> Void

    private static let timeFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: iconName)
                    .font(.caption)
                    .foregroundColor(iconColor)
                    .frame(width: 16)
                Text(item.title)
                    .font(.caption.weight(.semibold))
                    .foregroundColor(.primary)
                if item.kind == .agentTask, let status = item.agent?.status {
                    statusPill(status)
                }
                Spacer()
                Text(Self.timeFormatter.string(from: item.timestamp))
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }

            if item.kind == .agentTask, let taskId = item.agent?.taskId {
                Text(taskId)
                    .font(.caption2.monospaced())
                    .foregroundColor(.secondary.opacity(0.7))
                    .lineLimit(1)
            }

            Text(displayBody)
                .font(.caption)
                .foregroundColor(.secondary)
                .lineLimit(expanded ? nil : 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)

            if item.hasExpandableDetail || item.text.count > 120 {
                Button(expanded ? "Show less" : "Show more") {
                    onToggle()
                }
                .font(.caption2)
                .buttonStyle(.plain)
                .foregroundColor(.accentColor)
            }
        }
        .padding(10)
        .background(backgroundColor)
        .cornerRadius(10)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(item.title). \(displayBody)")
        .accessibilityIdentifier(item.kind == .agentTask ? "agentTimelineRow" : "timelineRow")
    }

    private func statusPill(_ status: String) -> some View {
        Text(status.replacingOccurrences(of: "_", with: " "))
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(iconColor.opacity(0.15))
            .foregroundColor(iconColor)
            .clipShape(Capsule())
    }

    private var displayBody: String {
        if expanded, let detail = item.detail {
            return detail
        }
        return item.text
    }

    private var iconName: String {
        switch item.kind {
        case .tool: return "wrench.and.screwdriver"
        case .system: return "bell"
        case .agentTask: return "cpu"
        case .error: return "exclamationmark.triangle.fill"
        case .mcp: return "server.rack"
        case .classification: return "brain"
        }
    }

    private var iconColor: Color {
        switch item.kind {
        case .tool: return .blue
        case .system: return .secondary
        case .agentTask:
            switch item.agent?.status {
            case "failed": return .red
            case "completed": return .green
            case "permission", "permission_resolved": return .orange
            default: return .purple
            }
        case .error: return .red
        case .mcp: return .teal
        case .classification: return .indigo
        }
    }

    private var backgroundColor: Color {
        switch item.kind {
        case .error:
            return Color.red.opacity(0.08)
        case .agentTask where item.agent?.status == "permission":
            return Color.orange.opacity(0.08)
        default:
            return Color(.secondarySystemBackground)
        }
    }
}

#Preview {
    NavigationStack {
        TimelineView()
            .environmentObject(CompanionViewModel())
    }
}
