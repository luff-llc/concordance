package com.example.fixture.home

import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import com.example.fixture.R

@Composable
fun HomeScreen(items: List<String>) {
    if (items.isEmpty()) Text(stringResource(R.string.home_empty))
    LazyColumn(Modifier.testTag("home.list")) {
        items(items.size) { index -> Text(items[index], Modifier.testTag("home.row.${items[index]}")) }
    }
}
