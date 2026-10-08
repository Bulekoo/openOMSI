use super::*;

#[test]
fn imported_millions_cannot_become_a_local_population_budget() {
    assert_eq!(bounded_people_limit(200), 200);
    assert_eq!(bounded_people_limit(1), 1);
    assert_eq!(bounded_people_limit(0), 1);
    assert_eq!(bounded_people_limit(MAX_LOCAL_PEOPLE), MAX_LOCAL_PEOPLE);
    assert_eq!(bounded_people_limit(2_000_000), MAX_LOCAL_PEOPLE);
    assert_eq!(bounded_people_limit(usize::MAX), MAX_LOCAL_PEOPLE);
}

#[test]
fn the_pool_counts_local_people_and_never_recycles_riders_or_avatars() {
    let dir = std::env::temp_dir().join(format!("omsi-pool-limit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("person.hum"), "[model]\nmodel.cfg\n").unwrap();
    std::fs::write(dir.join("model.cfg"), "").unwrap();
    let ty = Arc::new(HumanType::load(&dir.join("person.hum")).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
    let person = |id, remote, puppet, standing| Person {
        id,
        ty: ty.clone(),
        variant: 0,
        meshes: Vec::new(),
        position: DVec3::Y * 1000.0,
        heading: 0.0,
        lheading: 0.0,
        place: Place::Ground,
        vel: DVec2::ZERO,
        pace: 1.1,
        activity: Activity::Stand,
        anim: OmsiAnim::default(),
        state: if standing {
            State::Standing
        } else {
            State::Pax(Box::new(Pax::new(1.1, 0.5)))
        },
        t_state: 0.0,
        skins: Vec::new(),
        skin_bones: None,
        pose_changed: false,
        interior: 0.0,
        lit: 0.0,
        tilt: Mat4::IDENTITY,
        age: 40.0,
        stuck: 0.0,
        ghost: 0.0,
        car_wait: 0.0,
        detour: 0.0,
        detour_side: 0.0,
        why: "",
        skinned: false,
        since_posed: 0,
        posed_at: (DVec3::ZERO, 0.0),
        ankles: [Vec3::ZERO; 2],
        puppet,
        remote,
    };
    let mut h = PeopleSim::new(Path::new("/nonexistent"), 200);
    h.max_people = 2;
    h.people = vec![
        person(1, false, None, false),
        person(2, false, None, false),
        person(3, true, None, true),
        person(
            4,
            false,
            Some(Puppet {
                mode: PuppetMode::Avatar,
            }),
            true,
        ),
    ];
    assert_eq!(h.pool_used(), 2);
    assert!(!h.pool_room());
    assert_eq!(
        h.people.len(),
        4,
        "avatars, mirrors and existing passengers survive"
    );
    h.people.push(person(5, false, None, true));
    assert!(
        !h.pool_room(),
        "recycling one walker cannot grant a spawn when still over budget"
    );
    assert_eq!(h.pool_used(), 2);
    assert!(h.people.iter().all(|p| p.id != 5));
    h.people.retain(|p| p.id != 2);
    h.people.push(person(6, false, None, true));
    assert!(
        h.pool_room(),
        "an offscreen walker may make room at the exact limit"
    );
    assert_eq!(h.pool_used(), 1);
    assert!(h.people.iter().any(|p| p.id == 1));
}
