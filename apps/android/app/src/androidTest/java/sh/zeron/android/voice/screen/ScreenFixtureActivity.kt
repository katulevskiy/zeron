package sh.zeron.android.voice.screen

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.view.WindowManager
import android.widget.*

/** Native fixture lives in the separate test APK, never in the user build. */
class ScreenFixtureActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN)
        if (intent.getBooleanExtra("secure", false)) window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        val column = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24,24,24,24); setBackgroundColor(Color.rgb(210,245,245)) }
        val status = TextView(this).apply { text = "Task pending"; textSize = 24f }
        column.addView(Button(this).apply { text = "Mark complete"; setOnClickListener { status.text = "Task completed" } })
        column.addView(Button(this).apply { text = "Coordinate tap"; setOnClickListener { status.text = "Coordinate tapped" } })
        column.addView(EditText(this).apply { hint = "Search query"; contentDescription = "Search query" })
        column.addView(EditText(this).apply { hint = "Password"; inputType = 129; setText("DO_NOT_SEND_PASSWORD") })
        repeat(40) { column.addView(TextView(this).apply { text = "Scrollable row $it"; textSize = 20f; setPadding(0,20,0,20) }) }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            isFocusableInTouchMode = true
            setPadding(0, 150, 0, 0)
            setBackgroundColor(Color.rgb(210,245,245))
            addView(status)
            addView(ScrollView(this@ScreenFixtureActivity).apply { addView(column) }, LinearLayout.LayoutParams(-1, 0, 1f))
        })
    }
}
