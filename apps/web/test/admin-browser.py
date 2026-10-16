"""Browser acceptance against local Auth/Firestore emulators and the real catalog handler.

Requires scripts/seed-emulator-user.mjs, dev-server.mjs, and Vite on 5186.
Install Python playwright and point CHROME_EXECUTABLE to an installed Chrome.
No live Firebase project or real user credentials are used.
"""
import json
import os
import re
import urllib.request
from playwright.sync_api import sync_playwright, expect

BASE = 'http://127.0.0.1:5186'
API = 'http://127.0.0.1:4700'
PASSWORD = 'LocalOnly-2026-Admin!'

def login(page, email):
    page.goto(BASE + '/admin/models')
    page.wait_for_load_state('networkidle')
    page.get_by_label('邮箱', exact=True).fill(email)
    page.get_by_label('密码', exact=True).fill(PASSWORD)
    page.get_by_role('button', name='登录', exact=True).click()

def catalog(page, version='2.0.0', etag=None):
    return page.request.get(API + '/v1/model-catalog?platform=android&appVersion=' + version + '&schemaVersion=1',
        headers={'If-None-Match': etag} if etag else {})

def published_name(page):
    data = catalog(page).json()
    return next(m['name'] for m in data['models'] if m['providerId'] == 'anthropic-chat' and m['id'] == 'claude-fable-5')

with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8089/emulator/v1/projects/demo-lingxi-backend/databases/model-catalog/documents', method='DELETE')):
    pass

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, executable_path=os.environ.get('CHROME_EXECUTABLE', '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'))
    context = browser.new_context(viewport={'width': 1600, 'height': 1000})
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(BASE + '/admin/models')
    page.wait_for_load_state('networkidle')
    print('Login controls:', page.get_by_role('button').all_text_contents(), flush=True)
    page.screenshot(path='/private/tmp/lingxi-firebase-admin-login.png', full_page=True)
    login(page, 'admin@example.com')
    expect(page.locator('.catalog-model-link').first).to_be_visible(timeout=30000)
    assert page.locator('.catalog-model-link').count() == 40
    expect(page.get_by_text('1327 个模型', exact=False)).to_be_visible()
    assert catalog(page).status == 404
    page.locator('.catalog-model-link').first.click()
    page.get_by_label('显示名称', exact=True).fill('验收模型 A')
    page.get_by_label('最低版本（含）', exact=True).fill('2.0.0')
    page.get_by_label('最高版本（含）', exact=True).fill('3.0.0')
    page.get_by_role('button', name='保存草稿', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('草稿已保存', timeout=30000)
    page.get_by_role('button', name='版本预览', exact=True).click()
    page.get_by_label('平台', exact=True).select_option('android')
    page.get_by_label('应用版本', exact=True).fill('1.0.0')
    page.get_by_role('button', name='预览当前草稿', exact=True).click()
    expect(page.locator('.catalog-preview-summary')).to_be_visible(timeout=30000)
    assert page.get_by_role('cell', name='验收模型 A', exact=True).count() == 0
    page.get_by_role('button', name='发布历史', exact=True).click()
    page.get_by_label('发布说明', exact=True).fill('Emulator acceptance A')
    page.get_by_role('button', name='发布目录', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('已发布', timeout=30000)
    assert published_name(page) == '验收模型 A'
    low = catalog(page, '1.0.0').json()
    assert not any(m['id'] == 'claude-fable-5' and m['providerId'] == 'anthropic-chat' for m in low['models'])
    first = catalog(page)
    assert catalog(page, etag=first.headers['etag']).status == 304
    page.get_by_role('button', name=re.compile('^模型目录')).click()
    page.locator('.catalog-model-link').first.click()
    page.get_by_label('显示名称', exact=True).fill('验收模型 B')
    page.get_by_role('button', name='保存草稿', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('草稿已保存', timeout=30000)
    assert published_name(page) == '验收模型 A'
    page.get_by_role('button', name='发布历史', exact=True).click()
    page.get_by_label('发布说明', exact=True).fill('Emulator acceptance B')
    page.get_by_role('button', name='发布目录', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('已发布', timeout=30000)
    assert published_name(page) == '验收模型 B'
    article = page.locator('.catalog-release-list article').filter(has=page.get_by_role('heading', name='Emulator acceptance A', exact=True))
    article.get_by_role('button', name='恢复到草稿', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('已恢复到草稿', timeout=30000)
    assert published_name(page) == '验收模型 B'
    page.get_by_label('发布说明', exact=True).fill('Emulator restored A')
    page.get_by_role('button', name='发布目录', exact=True).click()
    expect(page.get_by_role('status')).to_contain_text('已发布', timeout=30000)
    assert published_name(page) == '验收模型 A'
    page.get_by_role('button', name='导入与导出', exact=True).click()
    page.get_by_label('Catalog JSON', exact=True).fill(json.dumps({'schemaVersion': 1, 'providers': [], 'models': [None]}))
    page.get_by_role('button', name='应用 JSON 到草稿', exact=True).click()
    expect(page.get_by_role('alert')).to_contain_text('models.0', timeout=30000)
    page.get_by_role('button', name=re.compile('^模型目录')).click()
    expect(page.locator('.catalog-model-link').first).to_be_visible()
    page.get_by_label('搜索模型', exact=True).fill('验收模型 A')
    expect(page.locator('.catalog-model-link')).to_have_count(1)
    page.locator('.catalog-model-link').first.click()
    page.screenshot(path='/private/tmp/lingxi-firebase-admin-desktop.png', full_page=True)
    page.reload()
    expect(page.locator('.catalog-model-link').first).to_be_visible(timeout=30000)
    page.set_viewport_size({'width': 390, 'height': 844})
    page.screenshot(path='/private/tmp/lingxi-firebase-admin-mobile.png', full_page=True)
    assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'Mobile document overflow'
    viewer = browser.new_context(viewport={'width': 1280, 'height': 800}).new_page()
    login(viewer, 'viewer@example.com')
    expect(viewer.get_by_role('alert')).to_contain_text('ADMIN_REQUIRED', timeout=30000)
    assert viewer.locator('.catalog-model-link').count() == 0
    assert not errors, errors
    print('PASS: real emulator login, pagination, edit/save, version preview, immutable release, ETag, restore/publish, JSON rejection, session restore, mobile layout, and ordinary-user denial.', flush=True)
    browser.close()
