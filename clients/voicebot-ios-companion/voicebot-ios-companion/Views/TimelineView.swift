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
                }
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 40))
                .foregroundColor(.secondary.opacity(0.5))
            Text("No events yet")
                .font(.headline)
                .foregroundColor(.secondary)
            Text("Tool calls, system notices, and agent activity appear here")
                .font(.caption)
                .foregroundColor(.secondary.opacity(0.7))
                .multilineTextAlignment(.center)
                .padding(.horizontal)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityIdentifier("timelineEmpty")
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
                Spacer()
                Text(Self.timeFormatter.string(from: item.timestamp))
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }

            Text(displayBody)
                .font(.caption)
                .foregroundColor(.secondary)
                .lineLimit(expanded ? nil : 3)
                .frame(maxWidth: .infinity, alignment: .leading)

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
