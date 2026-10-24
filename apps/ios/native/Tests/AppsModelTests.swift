import XCTest
@testable import LingxiCode

final class AppsModelTests: XCTestCase {
    private func app(_ id:String,name:String,removed:Bool = false) -> UnifiedAppRecord {
        .init(appId:id,name:name,summary:"",currentRevision:1,latestRevision:1,authoring:nil,marketOrigin:nil,originPort:32100,dataSchemaVersion:1,executionGrants:[],createdAtMs:1,updatedAtMs:1,removed:removed)
    }
    func testLibraryIdentityDoesNotDependOnSourceOrMarketPrefix() {
        let rows = AppsModel.myApps([app("local-a",name:"Beta"),app("local-b",name:"Alpha")],info:[:])
        XCTAssertEqual(rows.map(\.appId),["local-b","local-a"])
        XCTAssertFalse(rows.contains { $0.appId.hasPrefix("chat:") })
    }
    func testRemovedRowsNeverResurrectFromPresentationHistory() {
        let rows = AppsModel.myApps([app("gone",name:"Gone",removed:true)],info:["gone":.init(appId:"gone",name:"Gone",lastOpenedAt:Date())])
        XCTAssertTrue(rows.isEmpty)
    }
    func testRecentAppsUseStableLocalIdentityAndOpenTime() {
        let old = Date(timeIntervalSince1970:1), fresh = Date(timeIntervalSince1970:2)
        let rows = AppsModel.myApps([app("a",name:"A"),app("b",name:"B"),app("c",name:"C")],info:["b":.init(appId:"b",name:"Old cached title",lastOpenedAt:old),"c":.init(appId:"c",name:"C",lastOpenedAt:fresh)])
        XCTAssertEqual(AppsModel.recentApps(rows).map(\.appId),["c","b"])
        XCTAssertEqual(rows.first { $0.appId == "b" }?.name,"B")
    }
    func testSearchAndInitials() {
        let rows = AppsModel.myApps([app("a",name:"Water Tracker"),app("b",name:"记账本")],info:[:])
        XCTAssertEqual(AppsModel.matchingApps(rows,query:"water").map(\.appId),["a"])
        XCTAssertEqual(AppsModel.matchingApps(rows,query:"账").map(\.appId),["b"])
        XCTAssertTrue(AppsModel.matchingApps(rows,query:" ").isEmpty)
        XCTAssertEqual(AppsModel.initial("  calculator"),"C")
        XCTAssertEqual(AppsModel.initial(" "),"?")
    }
}
