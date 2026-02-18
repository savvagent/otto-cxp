# @cxp/kotlin — CXP Android SDK

Stub implementation for the CXP Customer Experience Platform Android SDK.

## Status

**v2 — Not yet implemented.** This package contains the public API surface for the CXP SDK. The implementation will connect to the CXP MCP server via streamable HTTP transport.

## Installation

Add to your `build.gradle.kts`:

```kotlin
dependencies {
    implementation("com.cxp:sdk:0.1.0")
}
```

## Usage

```kotlin
import com.cxp.sdk.CxpClient

val client = CxpClient("http://localhost:3000/mcp")
val customer = client.getCustomer("...")
```
