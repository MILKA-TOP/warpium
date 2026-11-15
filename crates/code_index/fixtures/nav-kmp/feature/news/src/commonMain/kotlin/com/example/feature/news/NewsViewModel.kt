package com.example.feature.news

import com.example.data.NewsRepository
import com.example.data.platformName
import com.example.model.*

class NewsViewModel(private val newsRepository: NewsRepository) {
    private val greeting = "Hello"

    fun load(): List<NewsResource> {
        val items = newsRepository.getNewsResources()
        val first: NewsResource = items.first()
        println("$greeting ${first.title}")
        return items.filter { it.id != platformName() }
    }

    companion object {
        const val PAGE_SIZE = 20
    }
}

fun summaries(resources: List<NewsResource>): List<String> = resources.map(NewsResource::summary)
