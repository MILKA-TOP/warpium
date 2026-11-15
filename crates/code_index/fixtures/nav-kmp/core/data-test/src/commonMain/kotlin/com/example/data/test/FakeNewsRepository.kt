package com.example.data.test

import com.example.data.NewsRepository
import com.example.model.NewsResource

class FakeNewsRepository : NewsRepository {
    override fun getNewsResources(): List<NewsResource> = emptyList()
}
