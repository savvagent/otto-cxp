# @cxp/swift — CXP iOS SDK

Stub implementation for the CXP Customer Experience Platform iOS SDK.

## Status

**v2 — Not yet implemented.** This package contains the public API surface for the CXP SDK. The implementation will connect to the CXP MCP server via streamable HTTP transport.

## Installation

Add to your `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/your-org/cxp", from: "0.1.0")
]
```

## Usage

```swift
import CxpSdk

let client = CxpClient(baseURL: URL(string: "http://localhost:3000/mcp")!)
let customer = try await client.getCustomer(id: "...")
```
