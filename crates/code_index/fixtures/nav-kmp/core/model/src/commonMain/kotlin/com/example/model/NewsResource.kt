package com.example.model

data class NewsResource(val id: String, val title: String) {
    fun summary(): String = title.take(10)
}

enum class Topic { ANDROID, KOTLIN }
