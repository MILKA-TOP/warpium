package com.example.shared

import kotlinx.coroutines.flow.StateFlow
import com.example.shared.model.*

expect class Platform() {
    val name: String
}

expect fun platformName(): String

class Greeting(private val platform: Platform) {
    fun greet(): String = "Hello, ${platform.name}!"

    companion object {
        const val DEFAULT_GREETING = "Hello"
    }
}

internal class UserViewModel(val state: StateFlow<String>) {
    private var counter = 0

    fun refresh() {
        fun local() {}
        local()
    }
}

fun String.shout(): String = uppercase()
