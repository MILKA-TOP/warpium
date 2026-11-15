package com.example.data

import com.example.model.NewsResource

interface NewsRepository {
    fun getNewsResources(): List<NewsResource>
}

class OfflineFirstNewsRepository(private val local: LocalSource) : NewsRepository {
    override fun getNewsResources(): List<NewsResource> = local.load()
}

class LocalSource {
    fun load(): List<NewsResource> = emptyList()
}
