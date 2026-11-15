// Error classes of the facade (spec 7.1, 7.4, 7.6).

/** Thrown or rejected by every call on an instance after a kernel trap (EVOL D8). */
export class KernelPoisonedError extends Error {
  constructor(message = "sandbox instance failed: the kernel trapped and the instance is unusable") {
    super(message);
    this.name = "KernelPoisonedError";
  }
}

/** `execSync` was asked to run with an asynchronous capability configured. */
export class AsyncCapabilityError extends Error {
  constructor(message = "execSync cannot be used with an asynchronous capability (custom command, custom fs, fetch, network or sleep)") {
    super(message);
    this.name = "AsyncCapabilityError";
  }
}

/** The feature is not part of this package (`transform()`, `registerTransformPlugin()`, lazy files for now). */
export class NotSupportedError extends Error {
  constructor(message = "not supported") {
    super(message);
    this.name = "NotSupportedError";
  }
}

/** Raised by the wrappers of a revoked scope (EVOL D9). */
export class ExecutionAbortedError extends Error {
  constructor(message = "execution aborted: the capability scope was revoked") {
    super(message);
    this.name = "ExecutionAbortedError";
  }
}
