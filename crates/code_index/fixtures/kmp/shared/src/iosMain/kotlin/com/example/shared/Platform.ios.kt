package com.example.shared

actual class Platform actual constructor() {
    actual val name: String = "iOS"
}

actual fun platformName(): String = "iOS"
