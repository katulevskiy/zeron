package sh.zeron.android

import android.app.Application
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.Fonts
import sh.zeron.android.design.SvgIcons

class ZeronApp : Application() {
    /** One model per process: the Rust client outlives activities. */
    lateinit var model: AppModel
        private set

    override fun onCreate() {
        super.onCreate()
        Fonts.init(this)
        SvgIcons.init(this)
        model = AppModel(this)
    }
}
