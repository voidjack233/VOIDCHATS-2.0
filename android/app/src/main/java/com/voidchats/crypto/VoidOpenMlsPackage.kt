package com.voidchats.crypto

import com.facebook.react.ReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.uimanager.ViewManager
import com.voidchats.storage.AppStorage
import com.voidchats.storage.VoidStorageModule

class VoidOpenMlsPackage : ReactPackage {
  override fun createNativeModules(context: ReactApplicationContext): List<NativeModule> {
    val storage = AppStorage(context)
    return listOf(VoidOpenMlsModule(context, storage), VoidStorageModule(context, storage))
  }

  override fun createViewManagers(context: ReactApplicationContext): List<ViewManager<*, *>> = emptyList()
}
