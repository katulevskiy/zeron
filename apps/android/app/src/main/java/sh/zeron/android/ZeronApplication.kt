package sh.zeron.android

import android.app.Application
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.Fonts
import sh.zeron.android.design.LocalAssets

class ZeronApplication : Application() {
    private var initializedModel: AppModel? = null
    val existingModel: AppModel? get() = initializedModel
    // Android keeps the selected assistant's service alive. Do not boot an
    // engine, load the native font layout library, or connect an account until
    // a user actually opens an activity or invokes the assistant UI.
    val model: AppModel get() = initializedModel ?: run {
        Fonts.init(assets)
        AppModel(this).also { initializedModel = it }
    }

    override fun onCreate() {
        super.onCreate()
        LocalAssets.manager = assets
    }
}
