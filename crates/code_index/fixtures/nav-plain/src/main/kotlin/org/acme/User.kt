package org.acme

data class User(val id: String, val name: String)

fun User.initials(): String = name.take(2)
