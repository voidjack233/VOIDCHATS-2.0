package com.voidchats.storage

import com.facebook.react.ReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.uimanager.ViewManager

class VoidNativePackage : ReactPackage {
  override fun createNativeModules(context: ReactApplicationContext): List<NativeModule> {
    val storage = AppStorage(context)
    return listOf(VoidStorageModule(context, storage), VoidMessagingModule(context, storage))
  }
  override fun createViewManagers(context: ReactApplicationContext): List<ViewManager<*, *>> = emptyList()
}
