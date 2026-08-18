package com.lingxi.code.localapps.widget

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.ComponentName
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.AdapterView
import android.widget.BaseAdapter
import android.widget.ListView
import android.widget.TextView
import androidx.activity.ComponentActivity
import com.lingxi.code.R

class LocalAppWidgetConfigureActivity : ComponentActivity() {
    private var appWidgetId: Int = AppWidgetManager.INVALID_APPWIDGET_ID
    private var bound = false
    private var pickerShown = false
    private val optionsRetryHandler = Handler(Looper.getMainLooper())
    private var optionsRetryAttempt = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setResult(Activity.RESULT_CANCELED)
        appWidgetId = intent?.extras?.getInt(
            AppWidgetManager.EXTRA_APPWIDGET_ID,
            AppWidgetManager.INVALID_APPWIDGET_ID,
        ) ?: AppWidgetManager.INVALID_APPWIDGET_ID
        if (appWidgetId == AppWidgetManager.INVALID_APPWIDGET_ID) {
            finish()
            return
        }
        title = getString(R.string.local_apps_title)
        applyConfigureStep()
    }

    override fun onDestroy() {
        optionsRetryHandler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private fun ownedWidgetIds(): IntArray =
        AppWidgetManager.getInstance(this)
            .getAppWidgetIds(ComponentName(this, LocalAppWidgetProvider::class.java))

    // Existing binding first, then this widget's options. Never read
    // EXTRA_APP_ID from the incoming Intent on this exported activity.
    // Wait never shows the picker: unknown ids retry, then finish.
    private fun applyConfigureStep() {
        if (bound || isFinishing) return
        val ownedIds = ownedWidgetIds()
        val retriesRemaining =
            optionsRetryAttempt < LocalAppWidgetPinRequester.OPTIONS_RETRY_ATTEMPTS
        val step = decideConfigureStep(
            ownership = widgetOwnership(appWidgetId, ownedIds),
            existingAppId = LocalAppWidgetSelectionStore.readAppId(this, appWidgetId),
            widgetScopedAppId = LocalAppWidgetPinRequester.appIdFromOptions(
                AppWidgetManager.getInstance(this).getAppWidgetOptions(appWidgetId),
            ),
            retriesRemaining = retriesRemaining,
        )
        when (step.action) {
            ConfigureAction.AutoBind -> {
                val appId = step.appId ?: return
                bindAndFinish(ownedIds, appId)
            }
            ConfigureAction.Finish -> finish()
            ConfigureAction.ShowPicker -> {
                if (!pickerShown) {
                    pickerShown = true
                    showPicker()
                }
                scheduleOptionsRetry()
            }
            ConfigureAction.Retry -> scheduleOptionsRetry()
        }
    }

    private fun scheduleOptionsRetry() {
        if (optionsRetryAttempt >= LocalAppWidgetPinRequester.OPTIONS_RETRY_ATTEMPTS) return
        optionsRetryHandler.postDelayed({
            if (bound || isFinishing || isDestroyed) return@postDelayed
            optionsRetryAttempt += 1
            applyConfigureStep()
        }, LocalAppWidgetPinRequester.OPTIONS_RETRY_INTERVAL_MS)
    }

    private fun showPicker() {
        setContentView(R.layout.local_app_widget_configure)

        val snapshot = LocalAppWidgetSnapshotStore.read(this)
        val listView = findViewById<ListView>(R.id.widget_app_list)
        val emptyView = findViewById<TextView>(R.id.widget_empty)
        listView.emptyView = emptyView
        listView.adapter = LocalAppWidgetListAdapter(layoutInflater, snapshot.apps)
        listView.onItemClickListener = AdapterView.OnItemClickListener { _, _, position, _ ->
            val app = snapshot.apps.getOrNull(position) ?: return@OnItemClickListener
            bindAndFinish(ownedWidgetIds(), app.id)
        }
    }

    private fun bindAndFinish(ownedIds: IntArray, appId: String) {
        if (bound) return
        when (widgetOwnership(appWidgetId, ownedIds)) {
            WidgetOwnership.Reject -> {
                finish()
                return
            }
            WidgetOwnership.Wait -> return
            WidgetOwnership.Accept -> Unit
        }
        bound = true
        optionsRetryHandler.removeCallbacksAndMessages(null)
        val manager = AppWidgetManager.getInstance(this)
        runCatching {
            LocalAppWidgetPinRequester.writeAppIdToOptions(manager, appWidgetId, appId)
        }
        LocalAppWidgetSelectionStore.writeAppId(this, appWidgetId, appId)
        LocalAppWidgetProvider.updateAppWidget(this, manager, appWidgetId)
        setResult(
            Activity.RESULT_OK,
            Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId),
        )
        finish()
    }
}

private class LocalAppWidgetListAdapter(
    private val inflater: LayoutInflater,
    private val apps: List<LocalAppWidgetApp>,
) : BaseAdapter() {
    override fun getCount(): Int = apps.size

    override fun getItem(position: Int): Any = apps[position]

    override fun getItemId(position: Int): Long = position.toLong()

    override fun getView(position: Int, convertView: View?, parent: ViewGroup): View {
        val view = convertView ?: inflater.inflate(
            R.layout.local_app_widget_configure_row,
            parent,
            false,
        )
        val app = apps[position]
        view.findViewById<TextView>(R.id.widget_row_title).text = app.name
        view.findViewById<TextView>(R.id.widget_row_brief).text = app.brief
        return view
    }
}
