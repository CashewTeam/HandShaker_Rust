import Foundation
import HandShakerFFI

// MARK: - Event stream

extension HandShakerRuntime {
    /// Subscribe to the backend event stream (queue-pull subscription,
    /// `hs_subscribe_events` + `hs_subscription_next`).
    ///
    /// The stream is driven by a detached `Task` whose blocking
    /// `hs_subscription_next` call (1000 ms timeout) runs on a dedicated
    /// serial `DispatchQueue` (round-2 P1-4) — it never occupies the actor
    /// or a cooperative executor worker, and the actor is never blocked by
    /// the poll. Semantics:
    ///  - Events are decoded as `EventEnvelope` and yielded with a
    ///    `.bufferingNewest(256)` policy (a slow consumer drops the oldest
    ///    buffered event rather than stalling the poller).
    ///  - A poll timeout is a no-op; polling continues.
    ///  - After `shutdown()` the runtime reports the `{"closed":true}`
    ///    sentinel and the stream finishes normally.
    ///  - Native failures (e.g. a lagged subscriber) finish the stream with
    ///    the thrown `HandShakerError`.
    ///  - `onTermination` cancels the poller, which releases the
    ///    subscription handle — cancelling the consuming task (or dropping
    ///    the stream) never leaks the native subscription.
    ///
    /// The subscription is created eagerly (here, not lazily on first
    /// iteration), so subscribe errors surface immediately; the poller
    /// itself only starts when the stream is iterated.
    public func eventStream() throws -> AsyncThrowingStream<EventEnvelope, Error> {
        let subscription = try handle.subscribe()
        // Round-2 P1-4: one serial queue per subscription; the poller
        // bridges its blocking native call into the async world via a
        // continuation, so no actor/cooperative executor is ever blocked.
        let pollQueue = DispatchQueue(label: "handshaker.events.\(ObjectIdentifier(handle))")
        let stream: AsyncThrowingStream<EventEnvelope, Error> = AsyncThrowingStream(
            // P1-5: buffer 256 instead of 1 — a slow UI consumer used to
            // silently drop lifecycle/terminal/warning events, not just
            // progress ticks.
            bufferingPolicy: .bufferingNewest(256)
        ) { continuation in
            let poller = Task.detached {
                defer { subscription.destroy() }
                var lastSequence: UInt64?
                while !Task.isCancelled {
                    do {
                        let outcome = try await withCheckedThrowingContinuation { bridge in
                            pollQueue.async {
                                bridge.resume(
                                    with: Result {
                                        try subscription.next(timeoutMs: 1000)
                                    }
                                )
                            }
                        }
                        // P1-4: .closed finishes unconditionally — no local
                        // shutdown flag involved, so a hub closed by Rust
                        // for any reason ends the stream instead of
                        // hot-polling.
                        switch outcome {
                        case .event(let data):
                            let envelope = try JSONDecoder().decode(
                                EventEnvelope.self,
                                from: data
                            )
                            // P1-5: sequence continuity — a gap means
                            // events were lost (broadcast lag or buffer
                            // drop); the stream can no longer be trusted,
                            // so it ends with an explicit gap error and the
                            // consumer re-subscribes / re-pulls state.
                            if let last = lastSequence,
                                envelope.sequence != last + 1
                            {
                                continuation.finish(throwing: HandShakerError.eventSequenceGap(
                                    "expected sequence \(last + 1), got \(envelope.sequence)"
                                ))
                                return
                            }
                            lastSequence = envelope.sequence
                            switch continuation.yield(envelope) {
                            case .enqueued:
                                break
                            case .dropped(let dropped):
                                // Consumer is slower than 256 events per
                                // poll cycle: surface the loss instead of
                                // pretending the stream is complete.
                                continuation.finish(throwing: HandShakerError.eventSequenceGap(
                                    "consumer dropped \(dropped) event(s) at sequence \(envelope.sequence)"
                                ))
                                return
                            case .terminated:
                                return // consumer cancelled the stream
                            @unknown default:
                                break
                            }
                        case .timeout:
                            continue // plain poll timeout — keep waiting
                        case .closed:
                            continuation.finish()
                            return
                        }
                    } catch {
                        continuation.finish(throwing: error)
                        return
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in poller.cancel() }
        }
        return stream
    }
}
