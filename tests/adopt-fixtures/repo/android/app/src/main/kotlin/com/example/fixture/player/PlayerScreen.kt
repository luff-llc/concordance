package com.example.fixture.player

import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTag
import androidx.compose.ui.res.stringResource
import com.example.fixture.R

@Composable
fun PlayerScreen(onPlay: () -> Unit) {
    Button(onClick = onPlay, modifier = Modifier.semantics { testTag = "player.play" }) {
        Text(stringResource(R.string.player_play))
    }
}
