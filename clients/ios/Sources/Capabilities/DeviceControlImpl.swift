// DeviceControlImpl.swift — iOS Local App status, haptics, and deep links.

import Foundation

#if canImport(UIKit)
    import UIKit
    import Contacts
    import EventKit
    import Network

    /// Native iOS implementation of the shared Local App device-control seam.
    final class DeviceControlImpl: IosDeviceControl, @unchecked Sendable {
        func statusJson() async throws -> String {
            let network = await Self.networkKind()
            return await MainActor.run {
                let device = UIDevice.current
                device.isBatteryMonitoringEnabled = true
                var status: [String: Any] = ["network": network]
                if device.batteryLevel >= 0 {
                    status["batteryPercent"] = Double(device.batteryLevel * 100)
                }
                switch device.batteryState {
                case .charging, .full: status["charging"] = true
                case .unplugged: status["charging"] = false
                case .unknown: break
                @unknown default: break
                }
                status["lowPowerMode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
                do {
                    let data = try JSONSerialization.data(withJSONObject: status)
                    return String(data: data, encoding: .utf8) ?? "{\"network\":\"unknown\"}"
                } catch {
                    return "{\"network\":\"unknown\"}"
                }
            }
        }

        private static func networkKind() async -> String {
            await withCheckedContinuation { (continuation: CheckedContinuation<String, Never>) in
                let monitor = NWPathMonitor()
                let lock = NSLock()
                var resumed = false
                let finish: (String) -> Void = { kind in
                    lock.lock()
                    defer { lock.unlock() }
                    guard !resumed else { return }
                    resumed = true
                    monitor.cancel()
                    continuation.resume(returning: kind)
                }
                monitor.pathUpdateHandler = { path in
                    finish(Self.kind(from: path))
                }
                monitor.start(queue: DispatchQueue(label: "com.lingxi.device-status"))
                DispatchQueue.global().asyncAfter(deadline: .now() + 1.5) {
                    finish("unknown")
                }
            }
        }

        private static func kind(from path: NWPath) -> String {
            if path.status != .satisfied {
                "offline"
            } else if path.usesInterfaceType(.wifi) {
                "wifi"
            } else if path.usesInterfaceType(.cellular) {
                "cellular"
            } else if path.usesInterfaceType(.wiredEthernet) {
                "ethernet"
            } else {
                "online"
            }
        }

        func triggerHaptic(style: String) async throws {
            try await MainActor.run {
                switch style {
                case "light":
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.prepare()
                    generator.impactOccurred()
                case "medium":
                    let generator = UIImpactFeedbackGenerator(style: .medium)
                    generator.prepare()
                    generator.impactOccurred()
                case "heavy":
                    let generator = UIImpactFeedbackGenerator(style: .heavy)
                    generator.prepare()
                    generator.impactOccurred()
                case "success":
                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                case "warning":
                    UINotificationFeedbackGenerator().notificationOccurred(.warning)
                case "error":
                    UINotificationFeedbackGenerator().notificationOccurred(.error)
                default:
                    throw DeviceControlFfiError.Rejected(message: "unsupported haptic style")
                }
            }
        }

        func openDeepLink(url: String) async throws {
            guard let target = URL(string: url) else {
                throw DeviceControlFfiError.Rejected(message: "invalid URL")
            }
            let canOpen = await MainActor.run {
                UIApplication.shared.canOpenURL(target)
            }
            guard canOpen else {
                throw DeviceControlFfiError.Other(message: "the system could not open the URL")
            }
            await MainActor.run {
                UIApplication.shared.open(target, options: [:], completionHandler: nil)
            }
        }

        func calendarJson(requestJson: String) async throws -> String {
            let request = try DeviceControlWireJSON.decodeCalendarQuery(requestJson)
            let store = EKEventStore()
            guard try await store.requestFullAccessToEvents() else {
                throw DeviceControlFfiError.Rejected(message: "calendar permission denied")
            }
            let start = Date(timeIntervalSince1970: TimeInterval(request.startMs) / 1000)
            let end = Date(timeIntervalSince1970: TimeInterval(request.endMs) / 1000)
            let events = store.events(matching: store.predicateForEvents(withStart: start, end: end, calendars: nil))
                .sorted { $0.startDate < $1.startDate }
                .prefix(Int(request.limit))
            let values = events.map { event in
                [
                    "id": event.eventIdentifier ?? "",
                    "title": event.title ?? "",
                    "start_ms": Int64(event.startDate.timeIntervalSince1970 * 1000),
                    "end_ms": Int64(event.endDate.timeIntervalSince1970 * 1000),
                    "all_day": event.isAllDay,
                    "location": event.location as Any,
                    "notes": event.notes as Any,
                    "calendar": event.calendar?.title as Any
                ] as [String: Any]
            }
            return try Self.encodeJSON(values)
        }

        func contactsJson(requestJson: String) async throws -> String {
            let request = try DeviceControlWireJSON.decodeContactsQuery(requestJson)
            let store = CNContactStore()
            guard try await store.requestAccess(for: .contacts) else {
                throw DeviceControlFfiError.Rejected(message: "contacts permission denied")
            }
            let keys: [CNKeyDescriptor] = [
                CNContactIdentifierKey as NSString,
                CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
                CNContactPhoneNumbersKey as NSString,
                CNContactEmailAddressesKey as NSString
            ]
            let contacts = try store.unifiedContacts(
                matching: CNContact.predicateForContacts(matchingName: request.query),
                keysToFetch: keys
            ).prefix(Int(request.limit))
            let values = contacts.map { contact in
                [
                    "id": contact.identifier,
                    "display_name": CNContactFormatter.string(from: contact, style: .fullName) ?? "",
                    "phones": contact.phoneNumbers.map { $0.value.stringValue },
                    "emails": contact.emailAddresses.map { String($0.value) }
                ] as [String: Any]
            }
            return try Self.encodeJSON(values)
        }

        private static func encodeJSON(_ value: Any) throws -> String {
            let data = try JSONSerialization.data(withJSONObject: value)
            guard let result = String(data: data, encoding: .utf8) else {
                throw DeviceControlFfiError.Other(message: "native JSON encoding failed")
            }
            return result
        }
    }
#endif

/// Host-owned calendar/contacts request contract. Rust `CalendarQuery` /
/// `ContactsQuery` serialize with snake_case field names.
enum DeviceControlWireJSON {
    struct CalendarQuery: Decodable {
        let startMs: UInt64
        let endMs: UInt64
        let limit: UInt32

        enum CodingKeys: String, CodingKey {
            case startMs = "start_ms"
            case endMs = "end_ms"
            case limit
        }
    }

    struct ContactsQuery: Decodable {
        let query: String
        let limit: UInt32
    }

    static func decodeCalendarQuery(_ json: String) throws -> CalendarQuery {
        try JSONDecoder().decode(CalendarQuery.self, from: Data(json.utf8))
    }

    static func decodeContactsQuery(_ json: String) throws -> ContactsQuery {
        try JSONDecoder().decode(ContactsQuery.self, from: Data(json.utf8))
    }
}
