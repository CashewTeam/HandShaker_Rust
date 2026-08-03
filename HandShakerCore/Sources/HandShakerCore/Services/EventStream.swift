import Foundation
import HandShakerFFI

// MARK: - Event stream

extension HandShakerRuntime {
    /// Subscribe to the backend event stream (queue-pull subscription,
    /// `hs_subscribe_events` + `hs_subscription_next`).
    ///
    /// The stream is driven by a background `Task` that polls
    /// `hs_subscription_next` with a 1000 ms timeout, so it never blocks
    /// the actor. Semantics:
    ///  - Events are decoded as `EventEnvelope` and yielded with a
    ///    `.bufferingNewest(1)` policy (a slow consumer drops the oldest
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
        let shutdownFlag = self.shutdownFlag
        let stream: AsyncThrowingStream<EventEnvelope, Error> = AsyncThrowingStream(
            bufferingPolicy: .bufferingNewest(1)
        ) { continuation in
            let poller = Task {
                defer { subscription.destroy() }
                while !Task.isCancelled {
                    do {
                        if let data = try subscription.next(timeoutMs: 1000) {
                            let envelope = try JSONDecoder().decode(
                                EventEnvelope.self,
                                from: data
                            )
                            continuation.yield(envelope)
                        } else if shutdownFlag.isSet {
                            // The `{"closed":true}` sentinel only appears
                            // after hs_runtime_shutdown, and shutdown()
                            // sets the flag before the FFI call — the
                            // stream is done.
                            continuation.finish()
                            return
                        }
                        // else: plain poll timeout — keep waiting.
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
